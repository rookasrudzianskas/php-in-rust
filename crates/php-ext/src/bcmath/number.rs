//! The final readonly class `BcMath\Number` (`bcmath.c`, 8.4).
//!
//! php-src declares `value` and `scale` as virtual properties served by
//! property handlers and renders `value` lazily. This port declares them as
//! ordinary public readonly properties and fills them (eagerly) whenever a
//! number is created, so the standard property machinery provides
//! `var_dump()`, `(array)`, `isset()`, `property_exists()` and the readonly
//! errors. The numeric state lives in [`NumberData`].
//!
//! Operator overloading goes through the `do_operation` handler (`+ - * / % **`
//! with another `Number`, a numeric string or an int, in either operand
//! position) and comparison through the `compare` handler.

use std::cmp::{max, min};

use php_runtime::class::{
    CastKind, ClassEntry, Function, OpKind, PropertyInfo, class_flags, member_flags,
};
use php_runtime::types::TypeDecl;
use php_runtime::{
    Args, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm,
};

use super::num::{self, BcNum, EXPAND_SCALE, RaiseModError, Sign};
use super::{
    INT_MAX, alloc_error_for, check_precision, check_result_size, memory_limit, raise_error,
    rounding_mode,
};

pub const NUMBER_CLASS: &str = "BcMath\\Number";

/// Native state of a `BcMath\Number` (`bcmath_number_obj_t`).
#[derive(Clone)]
pub struct NumberData {
    pub num: BcNum,
    /// The `scale` property: the number of fractional digits of `value`.
    pub scale: usize,
}

fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("BcMath\\Number method called without $this")
}

fn data(obj: &ObjRef) -> Option<NumberData> {
    obj.native::<NumberData>().map(|d| d.clone())
}

fn data_of(vm: &mut Vm, obj: &ObjRef) -> PResult<NumberData> {
    match data(obj) {
        Some(d) => Ok(d),
        None => Err(vm.throw_error(
            "Error",
            format!("Object of type {NUMBER_CLASS} is not initialized"),
        )),
    }
}

/// Stores the state and the `value` / `scale` properties.
fn store(obj: &ObjRef, num: BcNum, scale: usize) {
    obj.set_prop(b"value", Value::from(num.to_string_scale(scale)));
    obj.set_prop(b"scale", Value::Int(scale as i64));
    obj.set_native(NumberData { num, scale });
}

fn number_class(vm: &Vm) -> ClassRef {
    vm.find_class(NUMBER_CLASS.as_bytes())
        .expect("BcMath\\Number is registered")
}

/// `bcmath_number_new_obj`.
pub(crate) fn new_number(vm: &mut Vm, num: BcNum, scale: usize) -> PResult<ObjRef> {
    check_result_size(vm, num.len, scale)?;
    let ce = number_class(vm);
    let obj = vm.instantiate(&ce)?;
    store(&obj, num, scale);
    Ok(obj)
}

fn readonly_value_error(vm: &mut Vm) -> Flow {
    vm.throw_error(
        "Error",
        format!("Cannot modify readonly property {NUMBER_CLASS}::$value"),
    )
}

fn scale_range_error(vm: &mut Vm, arg_num: usize) -> Flow {
    vm.argument_value_error(arg_num, &format!("must be between 0 and {INT_MAX}"))
}

/// `bcmath_check_scale` for an optional `?int $scale` argument: the value
/// (0 for `null`) and whether it was `null`.
fn scale_param(vm: &mut Vm, args: &Args, idx: usize, arg_num: usize) -> PResult<(usize, bool)> {
    let is_null = args.is_null(idx);
    let lval = args.int_opt(idx).unwrap_or(0);
    if !(0..=INT_MAX).contains(&lval) {
        return Err(scale_range_error(vm, arg_num));
    }
    Ok((lval as usize, is_null))
}

// ---------------------------------------------------------------------------
// Operands
// ---------------------------------------------------------------------------

/// A `Number|string|int` operand.
enum Operand {
    Number(ObjRef),
    Str(PhpStr),
    Long(i64),
}

impl Operand {
    /// An argument after ZPP of `Number|string|int`.
    fn from_arg(v: &Value) -> Operand {
        match v {
            Value::Object(o) => Operand::Number(o.clone()),
            Value::String(s) => Operand::Str(s.clone()),
            Value::Int(l) => Operand::Long(*l),
            other => unreachable!("ZPP guarantees Number|string|int, got {other:?}"),
        }
    }

    /// `bc_num_from_obj_or_str_or_long`: the number and its full scale, or
    /// `None` for a malformed string.
    fn to_num(&self) -> Option<(BcNum, usize)> {
        match self {
            Operand::Number(o) => data(o).map(|d| (d.num, d.scale)),
            Operand::Str(s) => BcNum::parse(s.as_bytes(), 0, true),
            Operand::Long(l) => Some((BcNum::from_long(*l), 0)),
        }
    }
}

/// `bc_num_from_obj_or_str_or_long_with_err`.
fn operand_arg(vm: &mut Vm, v: &Value, arg_num: usize) -> PResult<(BcNum, usize)> {
    let Some((num, full_scale)) = Operand::from_arg(v).to_num() else {
        return Err(vm.argument_value_error(arg_num, "is not well-formed"));
    };
    if full_scale as u64 > INT_MAX as u64 {
        return Err(scale_range_error(vm, arg_num));
    }
    Ok((num, full_scale))
}

/// `bcmath_number_parse_num` for operators and comparisons: `None` means
/// the operand cannot be used (`FAILURE`).
fn parse_operand(vm: &mut Vm, v: &Value) -> PResult<Option<Operand>> {
    let v = v.unref();
    Ok(match &v {
        Value::Object(o) if o.class().instance_of_name(NUMBER_CLASS.as_bytes()) => {
            Some(Operand::Number(o.clone()))
        }
        Value::Int(l) => Some(Operand::Long(*l)),
        Value::String(s) => Some(Operand::Str(s.clone())),
        Value::Null | Value::Undef => None,
        other => vm.parse_arg_long_weak(other, false)?.map(Operand::Long),
    })
}

// ---------------------------------------------------------------------------
// Arithmetic with scale handling (`bcmath_number_*_internal`)
// ---------------------------------------------------------------------------

fn scale_too_large(vm: &mut Vm) -> Flow {
    vm.throw_value_error("scale of the result is too large")
}

/// `CHECK_RET_SCALE_OVERFLOW` on 64-bit platforms.
fn ret_scale_overflow(scale: usize) -> bool {
    scale as u64 > INT_MAX as u64
}

/// The operand numbers of an arithmetic operation with their full scales.
struct Operands<'a> {
    n1: &'a BcNum,
    n2: &'a BcNum,
    f1: usize,
    f2: usize,
}

fn add_internal(o: &Operands, scale: &mut usize, auto: bool) -> BcNum {
    if auto {
        *scale = max(o.f1, o.f2);
    }
    let mut ret = num::add(o.n1, o.n2, *scale);
    ret.truncate_scale(min(*scale, ret.scale));
    if ret.is_zero() {
        ret.sign = Sign::Plus;
    }
    ret.rm_trailing_zeros();
    ret
}

fn sub_internal(o: &Operands, scale: &mut usize, auto: bool) -> BcNum {
    if auto {
        *scale = max(o.f1, o.f2);
    }
    let mut ret = num::sub(o.n1, o.n2, *scale);
    ret.truncate_scale(min(*scale, ret.scale));
    if ret.is_zero() {
        ret.sign = Sign::Plus;
    }
    ret.rm_trailing_zeros();
    ret
}

fn mul_internal(vm: &mut Vm, o: &Operands, scale: &mut usize, auto: bool) -> PResult<BcNum> {
    if auto {
        *scale = o.f1 + o.f2;
        if ret_scale_overflow(*scale) {
            return Err(scale_too_large(vm));
        }
    }
    let mut ret = num::multiply(o.n1, o.n2, *scale);
    ret.truncate_scale(min(*scale, ret.scale));
    if ret.is_zero() {
        ret.sign = Sign::Plus;
    }
    ret.rm_trailing_zeros();
    Ok(ret)
}

fn div_internal(vm: &mut Vm, o: &Operands, scale: &mut usize, auto: bool) -> PResult<BcNum> {
    if auto {
        *scale = o.f1 + EXPAND_SCALE;
        if ret_scale_overflow(*scale) {
            return Err(scale_too_large(vm));
        }
    }
    check_result_size(vm, o.n1.len, *scale)?;
    let Some(mut ret) = num::divide(o.n1, o.n2, *scale) else {
        return Err(vm.throw_division_by_zero("Division by zero"));
    };
    ret.rm_trailing_zeros();
    if auto {
        let diff = *scale - ret.scale;
        *scale -= min(diff, EXPAND_SCALE);
    }
    Ok(ret)
}

fn mod_internal(vm: &mut Vm, o: &Operands, scale: &mut usize, auto: bool) -> PResult<BcNum> {
    if auto {
        *scale = max(o.f1, o.f2);
    }
    check_result_size(vm, max(o.n1.len, o.n2.len), *scale)?;
    let Some(mut ret) = num::modulo(o.n1, o.n2, *scale) else {
        return Err(vm.throw_division_by_zero("Modulo by zero"));
    };
    ret.rm_trailing_zeros();
    Ok(ret)
}

fn pow_internal(
    vm: &mut Vm,
    o: &Operands,
    scale: &mut usize,
    auto: bool,
    is_op: bool,
) -> PResult<BcNum> {
    // Check the exponent for scale digits and convert to a long.
    if o.n2.scale != 0 {
        let msg = "exponent cannot have a fractional part";
        return Err(if is_op {
            vm.throw_value_error(msg)
        } else {
            vm.argument_value_error(1, msg)
        });
    }
    let exponent = o.n2.to_long();

    let mut scale_expand = false;
    if auto {
        if exponent > 0 {
            *scale = (o.f1 as u64).wrapping_mul(exponent as u64) as usize;
            if *scale as u64 > INT_MAX as u64 || *scale < o.f1 {
                return Err(scale_too_large(vm));
            }
        } else if exponent < 0 {
            *scale = o.f1 + EXPAND_SCALE;
            if ret_scale_overflow(*scale) {
                return Err(scale_too_large(vm));
            }
            scale_expand = true;
        } else {
            *scale = 0;
        }
    }

    // bc_num2long() returns 0 if the exponent is too large.
    if exponent == 0 && (o.n2.len > 1 || o.n2.digits[0] != 0) {
        let msg = "exponent is too large";
        return Err(if is_op {
            vm.throw_value_error(msg)
        } else {
            vm.argument_value_error(1, msg)
        });
    }
    check_result_size(vm, o.n1.len, *scale)?;
    let limit = memory_limit(vm);
    let mut ret = match num::raise(o.n1, exponent, *scale, limit) {
        Ok(r) => r,
        Err(e) => return Err(raise_error(vm, e, if is_op { 0 } else { 1 })),
    };
    ret.rm_trailing_zeros();
    if scale_expand {
        let diff = *scale - ret.scale;
        *scale -= min(diff, EXPAND_SCALE);
    }
    Ok(ret)
}

/// Runs the arithmetic operation `op` (`bcmath_number_*_internal`).
fn arithmetic(
    vm: &mut Vm,
    op: OpKind,
    o: &Operands,
    scale: &mut usize,
    auto: bool,
    is_op: bool,
) -> PResult<BcNum> {
    match op {
        OpKind::Add => Ok(add_internal(o, scale, auto)),
        OpKind::Sub => Ok(sub_internal(o, scale, auto)),
        OpKind::Mul => mul_internal(vm, o, scale, auto),
        OpKind::Div => div_internal(vm, o, scale, auto),
        OpKind::Mod => mod_internal(vm, o, scale, auto),
        OpKind::Pow => pow_internal(vm, o, scale, auto, is_op),
        _ => unreachable!("not an arithmetic operator"),
    }
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    if data(&this).is_some() {
        return Err(readonly_value_error(vm));
    }
    let (num, scale) = operand_arg(vm, args.get(0), 1)?;
    check_result_size(vm, num.len, scale)?;
    store(&this, num, scale);
    Ok(Value::Null)
}

/// `bcmath_number_calc_method`.
fn calc_method(vm: &mut Vm, args: &mut Args, op: OpKind) -> PResult<Value> {
    let (num, num_full_scale) = operand_arg(vm, args.get(0), 1)?;
    let (mut scale, scale_is_null) = scale_param(vm, args, 1, 2)?;
    let intern = data_of(vm, &this(args))?;
    let operands = Operands {
        n1: &intern.num,
        n2: &num,
        f1: intern.scale,
        f2: num_full_scale,
    };
    let ret = arithmetic(vm, op, &operands, &mut scale, scale_is_null, false)?;
    Ok(Value::Object(new_number(vm, ret, scale)?))
}

fn add(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    calc_method(vm, args, OpKind::Add)
}
fn sub(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    calc_method(vm, args, OpKind::Sub)
}
fn mul(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    calc_method(vm, args, OpKind::Mul)
}
fn div(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    calc_method(vm, args, OpKind::Div)
}
fn modulo(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    calc_method(vm, args, OpKind::Mod)
}
fn pow(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    calc_method(vm, args, OpKind::Pow)
}

fn divmod(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (num, num_full_scale) = operand_arg(vm, args.get(0), 1)?;
    let (scale_lval, scale_is_null) = scale_param(vm, args, 1, 2)?;
    let intern = data_of(vm, &this(args))?;
    let scale = if scale_is_null {
        max(intern.scale, num_full_scale)
    } else {
        scale_lval
    };
    check_result_size(vm, max(intern.num.len, num.len), scale)?;
    let Some((mut quot, mut rem)) = num::divmod(&intern.num, &num, scale) else {
        return Err(vm.throw_division_by_zero("Division by zero"));
    };
    quot.rm_trailing_zeros();
    rem.rm_trailing_zeros();
    let quot = new_number(vm, quot, 0)?;
    let rem = new_number(vm, rem, scale)?;
    Ok(Value::array(PhpArray::from_values(vec![
        Value::Object(quot),
        Value::Object(rem),
    ])))
}

fn powmod(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (exponent, _) = operand_arg(vm, args.get(0), 1)?;
    let (modulus, _) = operand_arg(vm, args.get(1), 2)?;
    let (scale, _) = scale_param(vm, args, 2, 3)?;
    let intern = data_of(vm, &this(args))?;
    check_result_size(vm, max(intern.num.len, modulus.len), scale)?;
    let mut ret = match num::raisemod(&intern.num, &exponent, &modulus, scale) {
        Ok(r) => r,
        Err(RaiseModError::BaseHasFractional) => {
            return Err(vm.throw_value_error("Base number cannot have a fractional part"));
        }
        Err(RaiseModError::ExpoHasFractional) => {
            return Err(vm.argument_value_error(1, "cannot have a fractional part"));
        }
        Err(RaiseModError::ExpoIsNegative) => {
            return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
        }
        Err(RaiseModError::ModHasFractional) => {
            return Err(vm.argument_value_error(2, "cannot have a fractional part"));
        }
        Err(RaiseModError::ModIsZero) => {
            return Err(vm.throw_division_by_zero("Modulo by zero"));
        }
    };
    ret.rm_trailing_zeros();
    Ok(Value::Object(new_number(vm, ret, scale)?))
}

fn sqrt(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (scale_lval, scale_is_null) = scale_param(vm, args, 0, 1)?;
    let intern = data_of(vm, &this(args))?;
    let mut scale = if scale_is_null {
        let s = intern.scale + EXPAND_SCALE;
        if ret_scale_overflow(s) {
            return Err(scale_too_large(vm));
        }
        s
    } else {
        scale_lval
    };
    check_result_size(vm, intern.num.len, scale)?;
    let Some(mut ret) = num::sqrt(&intern.num, scale) else {
        return Err(vm.throw_value_error("Base number must be greater than or equal to 0"));
    };
    ret.truncate_scale(min(scale, ret.scale));
    ret.rm_trailing_zeros();
    if scale_is_null {
        let diff = scale - ret.scale;
        scale -= min(diff, EXPAND_SCALE);
    }
    Ok(Value::Object(new_number(vm, ret, scale)?))
}

fn compare_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (num, _) = operand_arg(vm, args.get(0), 1)?;
    let (scale_lval, scale_is_null) = scale_param(vm, args, 1, 2)?;
    let intern = data_of(vm, &this(args))?;
    let scale = if scale_is_null {
        max(intern.num.scale, num.scale)
    } else {
        scale_lval
    };
    Ok(Value::Int(i64::from(num::compare(
        &intern.num,
        &num,
        scale,
    ))))
}

fn floor_or_ceil(vm: &mut Vm, args: &Args, is_floor: bool) -> PResult<Value> {
    let intern = data_of(vm, &this(args))?;
    let ret = num::floor_or_ceil(&intern.num, is_floor);
    Ok(Value::Object(new_number(vm, ret, 0)?))
}

fn floor(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    floor_or_ceil(vm, args, true)
}

fn ceil(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    floor_or_ceil(vm, args, false)
}

fn round(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let precision = args.int(0);
    check_precision(vm, precision, 1)?;
    let mode = rounding_mode(vm, args.get(1), 2)?;
    let intern = data_of(vm, &this(args))?;
    let limit = memory_limit(vm);
    let (mut ret, scale) = match num::round(&intern.num, precision, mode, limit) {
        Ok(r) => r,
        Err(e) => return Err(alloc_error_for(vm, e)),
    };
    ret.rm_trailing_zeros();
    Ok(Value::Object(new_number(vm, ret, scale)?))
}

fn to_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    data_of(vm, &obj)?;
    Ok(obj.get_prop(b"value").unwrap_or(Value::Null))
}

fn serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    data_of(vm, &obj)?;
    let mut props = PhpArray::new();
    props.insert_str("value", obj.get_prop(b"value").unwrap_or(Value::Null));
    Ok(Value::array(props))
}

fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let fail = |vm: &mut Vm| {
        vm.throw_error(
            "Exception",
            format!("Invalid serialization data for {NUMBER_CLASS} object"),
        )
    };
    let value = match args.array(0).get_str(b"value") {
        Some(v) => v.unref(),
        None => return Err(fail(vm)),
    };
    let Value::String(value) = value else {
        return Err(fail(vm));
    };
    if value.is_empty() {
        return Err(fail(vm));
    }
    let obj = this(args);
    if data(&obj).is_some() {
        return Err(readonly_value_error(vm));
    }
    match BcNum::parse(value.as_bytes(), 0, true) {
        Some((num, scale)) if scale as u64 <= INT_MAX as u64 => {
            check_result_size(vm, num.len, scale)?;
            store(&obj, num, scale);
            Ok(Value::Null)
        }
        _ => Err(fail(vm)),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn create_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    Ok(ObjRef::new(ce.clone()))
}

/// `bcmath_number_clone`.
fn clone_object(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = vm.instantiate(&ce)?;
    if let Some(d) = data(old) {
        store(&new, d.num, d.scale);
    }
    Ok(new)
}

/// `bcmath_number_cast_object`: `(bool)` is "not zero", `(string)` the value.
fn cast_object(_vm: &mut Vm, obj: &ObjRef, kind: CastKind) -> PResult<Option<Value>> {
    let Some(d) = data(obj) else {
        return Ok(None);
    };
    Ok(match kind {
        CastKind::Bool => Some(Value::Bool(!d.num.is_zero())),
        CastKind::String => obj.get_prop(b"value"),
        _ => None,
    })
}

fn scale_overflow_error(vm: &mut Vm) -> Flow {
    vm.throw_value_error(format!("scale must be between 0 and {INT_MAX}"))
}

/// `bcmath_number_compare`: -1, 0 or 1; `1` also for operands that cannot
/// be compared (`ZEND_UNCOMPARABLE`).
fn compare_handler(vm: &mut Vm, a: &Value, b: &Value) -> PResult<i32> {
    let Some(op1) = parse_operand(vm, a)? else {
        return Ok(1);
    };
    let Some(op2) = parse_operand(vm, b)? else {
        return Ok(1);
    };
    let (Some((n1, f1)), Some((n2, f2))) = (op1.to_num(), op2.to_num()) else {
        return Ok(1);
    };
    if f1 as u64 > INT_MAX as u64 || f2 as u64 > INT_MAX as u64 {
        return Err(scale_overflow_error(vm));
    }
    Ok(num::compare(&n1, &n2, max(n1.scale, n2.scale)))
}

/// `bcmath_number_do_operation`: `Ok(None)` is `FAILURE` (the engine then
/// reports unsupported operand types).
fn do_operation(vm: &mut Vm, op: OpKind, a: &Value, b: &Value) -> PResult<Option<Value>> {
    if !matches!(
        op,
        OpKind::Add | OpKind::Sub | OpKind::Mul | OpKind::Div | OpKind::Mod | OpKind::Pow
    ) {
        return Ok(None);
    }
    let Some(op1) = parse_operand(vm, a)? else {
        return Ok(None);
    };
    let Some(op2) = parse_operand(vm, b)? else {
        return Ok(None);
    };
    let Some((n1, f1)) = op1.to_num() else {
        return Err(vm.throw_value_error(format!(
            "Left string operand cannot be converted to {NUMBER_CLASS}"
        )));
    };
    let Some((n2, f2)) = op2.to_num() else {
        return Err(vm.throw_value_error(format!(
            "Right string operand cannot be converted to {NUMBER_CLASS}"
        )));
    };
    if f1 as u64 > INT_MAX as u64 || f2 as u64 > INT_MAX as u64 {
        return Err(scale_overflow_error(vm));
    }

    let mut scale = 0;
    let operands = Operands {
        n1: &n1,
        n2: &n2,
        f1,
        f2,
    };
    let ret = arithmetic(vm, op, &operands, &mut scale, true, true)?;
    Ok(Some(Value::Object(new_number(vm, ret, scale)?)))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

macro_rules! method {
    ($name:ident, $sig:literal, $handler:ident) => {
        static $name: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

method!(
    CONSTRUCT,
    "public function __construct(string|int $num)",
    construct
);
method!(
    ADD,
    "public function add(\\BcMath\\Number|string|int $num, ?int $scale = null): \\BcMath\\Number",
    add
);
method!(
    SUB,
    "public function sub(\\BcMath\\Number|string|int $num, ?int $scale = null): \\BcMath\\Number",
    sub
);
method!(
    MUL,
    "public function mul(\\BcMath\\Number|string|int $num, ?int $scale = null): \\BcMath\\Number",
    mul
);
method!(
    DIV,
    "public function div(\\BcMath\\Number|string|int $num, ?int $scale = null): \\BcMath\\Number",
    div
);
method!(
    MOD,
    "public function mod(\\BcMath\\Number|string|int $num, ?int $scale = null): \\BcMath\\Number",
    modulo
);
method!(
    DIVMOD,
    "public function divmod(\\BcMath\\Number|string|int $num, ?int $scale = null): array",
    divmod
);
method!(
    POWMOD,
    "public function powmod(\\BcMath\\Number|string|int $exponent, \\BcMath\\Number|string|int $modulus, ?int $scale = null): \\BcMath\\Number",
    powmod
);
method!(
    POW,
    "public function pow(\\BcMath\\Number|string|int $exponent, ?int $scale = null): \\BcMath\\Number",
    pow
);
method!(
    SQRT,
    "public function sqrt(?int $scale = null): \\BcMath\\Number",
    sqrt
);
method!(FLOOR, "public function floor(): \\BcMath\\Number", floor);
method!(CEIL, "public function ceil(): \\BcMath\\Number", ceil);
method!(
    ROUND,
    "public function round(int $precision = 0, \\RoundingMode $mode = \\RoundingMode::HalfAwayFromZero): \\BcMath\\Number",
    round
);
method!(
    COMPARE,
    "public function compare(\\BcMath\\Number|string|int $num, ?int $scale = null): int",
    compare_method
);
method!(TO_STRING, "public function __toString(): string", to_string);
method!(SERIALIZE, "public function __serialize(): array", serialize);
method!(
    UNSERIALIZE,
    "public function __unserialize(array $data): void",
    unserialize
);

/// Registers `BcMath\Number`.
pub fn register(r: &mut Registry<'_>) {
    let mut ce = ClassEntry::new(NUMBER_CLASS, class_flags::FINAL | class_flags::READONLY);
    if let Some(stringable) = r.find_class("Stringable") {
        ce.interfaces.push(stringable);
    }

    for (name, ty) in [("value", "string"), ("scale", "int")] {
        let mut prop = PropertyInfo::new(
            name,
            NUMBER_CLASS,
            member_flags::PUBLIC | member_flags::READONLY,
        );
        prop.ty = Some(TypeDecl::parse(ty).expect("valid type"));
        prop.default = Value::Undef;
        ce.declare_property(prop);
    }

    for nf in [
        &CONSTRUCT,
        &ADD,
        &SUB,
        &MUL,
        &DIV,
        &MOD,
        &DIVMOD,
        &POWMOD,
        &POW,
        &SQRT,
        &FLOOR,
        &CEIL,
        &ROUND,
        &COMPARE,
        &TO_STRING,
        &SERIALIZE,
        &UNSERIALIZE,
    ] {
        ce.add_method(Function::native_method(
            nf,
            NUMBER_CLASS,
            member_flags::PUBLIC,
        ));
    }

    ce.handlers.create_object = Some(create_object);
    ce.handlers.clone_obj = Some(clone_object);
    ce.handlers.cast_object = Some(cast_object);
    ce.handlers.compare = Some(compare_handler);
    ce.handlers.do_operation = Some(do_operation);
    r.class(ce);
}
