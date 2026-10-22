//! Random engines: the `Random\Engine` interfaces, the native engine classes
//! (`Mt19937`, `PcgOneseq128XslRr64`, `Xoshiro256StarStar`, `Secure`), the
//! `Random\*` exception classes, and [`Engine`], the equivalent of
//! `php_random_algo_with_state` (a generator plus its state) used by every
//! consumer of random numbers.

use php_runtime::class::{ClassEntry, Function, class_flags, member_flags};
use php_runtime::{
    Args, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, Registry, Value, Vm,
};

use super::algo::{
    MT_RAND_MT19937, MT_RAND_PHP, Mt19937, Pcg128, RANGE_ATTEMPTS, RandResult, RandomGlobals,
    Xoshiro256, random_bytes, random_int,
};
use super::props::properties_load;

pub const ENGINE_INTERFACE: &str = "Random\\Engine";
pub const CRYPTO_SAFE_ENGINE_INTERFACE: &str = "Random\\CryptoSafeEngine";
pub const MT19937_CLASS: &str = "Random\\Engine\\Mt19937";
pub const PCG_CLASS: &str = "Random\\Engine\\PcgOneseq128XslRr64";
pub const XOSHIRO_CLASS: &str = "Random\\Engine\\Xoshiro256StarStar";
pub const SECURE_CLASS: &str = "Random\\Engine\\Secure";
pub const RANDOM_ERROR: &str = "Random\\RandomError";
pub const BROKEN_ENGINE_ERROR: &str = "Random\\BrokenRandomEngineError";
pub const RANDOM_EXCEPTION: &str = "Random\\RandomException";

/// The state of an internal engine object (`php_random_engine`).
#[derive(Clone)]
pub enum EngineState {
    Mt19937(Box<Mt19937>),
    Pcg(Pcg128),
    Xoshiro(Xoshiro256),
    /// `Random\Engine\Secure`: no state, the CSPRNG is used directly.
    Secure,
}

/// `php_random_algo_with_state`: a generator and its state.
#[derive(Clone)]
pub enum Engine {
    /// The global Mt19937 (`php_random_default_engine()`).
    Global,
    /// An internal engine object.
    Native(ObjRef),
    /// A userland `Random\Engine` implementation.
    User(ObjRef),
}

/// The per-VM globals of ext/random.
pub fn globals(vm: &mut Vm) -> &mut RandomGlobals {
    vm.ext_data::<RandomGlobals>()
}

/// `zend_throw_exception(random_ce_Random_RandomException, msg, 0)`.
pub fn throw_random_exception(vm: &mut Vm, msg: &str) -> Flow {
    vm.throw_error(RANDOM_EXCEPTION, msg)
}

/// A `php_random_bytes_throw` failure followed by the caller's
/// "Failed to generate a random seed" exception (chained like php-src).
fn throw_seed_failure(vm: &mut Vm, errstr: &str) -> Flow {
    let first = vm.create_exception(RANDOM_EXCEPTION, errstr, 0);
    let second = vm.create_exception(RANDOM_EXCEPTION, "Failed to generate a random seed", 0);
    second.set_prop(b"previous", Value::Object(first));
    Flow::Exception(second)
}

fn broken_engine(vm: &mut Vm) -> Flow {
    vm.throw_error(
        BROKEN_ENGINE_ERROR,
        format!("Failed to generate an acceptable random number in {RANGE_ATTEMPTS} attempts"),
    )
}

impl Engine {
    /// `algo->generate(state)`.
    pub fn generate(&self, vm: &mut Vm) -> PResult<RandResult> {
        match self {
            Engine::Global => Ok(globals(vm).default_mt().generate()),
            Engine::Native(obj) => native_generate(vm, obj),
            Engine::User(obj) => user_generate(vm, obj),
        }
    }

    /// `algo->range(state, min, max)`.
    pub fn range(&self, vm: &mut Vm, min: i64, max: i64) -> PResult<i64> {
        if let Engine::Native(obj) = self
            && matches!(
                obj.native::<EngineState>().as_deref(),
                Some(EngineState::Secure)
            )
        {
            return random_int(min, max).map_err(|e| throw_random_exception(vm, &e));
        }
        self.php_random_range(vm, min, max)
    }

    /// Whether this is an Mt19937 running the legacy `MT_RAND_PHP` variant.
    pub fn is_mt_php_mode(&self, vm: &mut Vm) -> bool {
        match self {
            Engine::Global => globals(vm).default_mt().mode != MT_RAND_MT19937,
            Engine::Native(obj) => matches!(
                obj.native::<EngineState>().as_deref(),
                Some(EngineState::Mt19937(s)) if s.mode != MT_RAND_MT19937
            ),
            Engine::User(_) => false,
        }
    }

    /// Fills a u32 from as many generator calls as needed.
    pub fn gen_u32(&self, vm: &mut Vm) -> PResult<u32> {
        let mut result = 0u32;
        let mut total = 0usize;
        loop {
            let r = self.generate(vm)?;
            result |= (r.result as u32) << (total * 8);
            total += r.size;
            if total >= 4 {
                return Ok(result);
            }
        }
    }

    pub fn gen_u64(&self, vm: &mut Vm) -> PResult<u64> {
        let mut result = 0u64;
        let mut total = 0usize;
        loop {
            let r = self.generate(vm)?;
            result |= r.result << (total * 8);
            total += r.size;
            if total >= 8 {
                return Ok(result);
            }
        }
    }

    /// `php_random_range32`.
    pub fn range32(&self, vm: &mut Vm, umax: u32) -> PResult<u32> {
        let mut result = self.gen_u32(vm)?;
        if umax == u32::MAX {
            return Ok(result);
        }
        let umax = umax + 1;
        if umax & (umax - 1) == 0 {
            return Ok(result & (umax - 1));
        }
        let limit = u32::MAX - (u32::MAX % umax) - 1;
        let mut count = 0u32;
        while result > limit {
            count += 1;
            if count > RANGE_ATTEMPTS {
                return Err(broken_engine(vm));
            }
            result = self.gen_u32(vm)?;
        }
        Ok(result % umax)
    }

    /// `php_random_range64`.
    pub fn range64(&self, vm: &mut Vm, umax: u64) -> PResult<u64> {
        let mut result = self.gen_u64(vm)?;
        if umax == u64::MAX {
            return Ok(result);
        }
        let umax = umax + 1;
        if umax & (umax - 1) == 0 {
            return Ok(result & (umax - 1));
        }
        let limit = u64::MAX - (u64::MAX % umax) - 1;
        let mut count = 0u32;
        while result > limit {
            count += 1;
            if count > RANGE_ATTEMPTS {
                return Err(broken_engine(vm));
            }
            result = self.gen_u64(vm)?;
        }
        Ok(result % umax)
    }

    /// `php_random_range`.
    pub fn php_random_range(&self, vm: &mut Vm, min: i64, max: i64) -> PResult<i64> {
        let umax = (max as u64).wrapping_sub(min as u64);
        if umax > u32::MAX as u64 {
            Ok(self.range64(vm, umax)?.wrapping_add(min as u64) as i64)
        } else {
            Ok((self.range32(vm, umax as u32)? as i64).wrapping_add(min))
        }
    }
}

fn uninitialized(vm: &mut Vm, class: &str) -> Flow {
    vm.throw_error(
        "Error",
        format!("Object of type {class} is not initialized"),
    )
}

fn native_generate(vm: &mut Vm, obj: &ObjRef) -> PResult<RandResult> {
    {
        let Some(mut st) = obj.native_mut::<EngineState>() else {
            let class = obj.class().name.to_string();
            return Err(uninitialized(vm, &class));
        };
        match &mut *st {
            EngineState::Mt19937(s) => return Ok(s.generate()),
            EngineState::Pcg(s) => return Ok(s.generate()),
            EngineState::Xoshiro(s) => return Ok(s.generate()),
            EngineState::Secure => {}
        }
    }
    // Random\Engine\Secure
    let mut b = [0u8; 8];
    match random_bytes(&mut b) {
        Ok(()) => Ok(RandResult {
            result: u64::from_le_bytes(b),
            size: 8,
        }),
        Err(e) => Err(throw_random_exception(vm, &e)),
    }
}

fn user_generate(vm: &mut Vm, obj: &ObjRef) -> PResult<RandResult> {
    let ret = vm.call_method(obj, b"generate", Vec::new())?.into_unref();
    let Value::String(s) = ret else {
        let msg = format!(
            "{}::generate(): Return value must be of type string, {} returned",
            obj.class().name,
            ret.type_name()
        );
        return Err(vm.throw_type_error(msg));
    };
    let size = s.len().min(8);
    if size == 0 {
        return Err(vm.throw_error(
            BROKEN_ENGINE_ERROR,
            "A random engine must return a non-empty string",
        ));
    }
    let mut result = 0u64;
    for (i, b) in s.as_bytes()[..size].iter().enumerate() {
        result += (*b as u64) << (8 * i);
    }
    Ok(RandResult { result, size })
}

// ---------------------------------------------------------------------------
// Object creation / cloning
// ---------------------------------------------------------------------------

fn new_engine_object(ce: &ClassRef, state: EngineState) -> ObjRef {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(state);
    obj
}

fn create_mt19937(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    Ok(new_engine_object(ce, EngineState::Mt19937(Box::default())))
}

fn create_pcg(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    Ok(new_engine_object(ce, EngineState::Pcg(Pcg128::default())))
}

fn create_xoshiro(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    Ok(new_engine_object(
        ce,
        EngineState::Xoshiro(Xoshiro256::default()),
    ))
}

fn create_secure(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    Ok(new_engine_object(ce, EngineState::Secure))
}

/// `php_random_engine_common_clone_object`.
fn clone_engine(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = vm.instantiate(&ce)?;
    if let Some(state) = old.native::<EngineState>().map(|s| s.clone()) {
        new.set_native(state);
    }
    for (name, value) in old.properties() {
        new.set_prop(&name, value);
    }
    Ok(new)
}

/// `clone_obj = NULL`: "Trying to clone an uncloneable object of class X".
pub fn clone_forbidden(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    Err(vm.throw_error(
        "Error",
        format!(
            "Trying to clone an uncloneable object of class {}",
            old.class().name
        ),
    ))
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("engine method called without $this")
}

fn state_missing(vm: &mut Vm, obj: &ObjRef) -> Flow {
    let class = obj.class().name.to_string();
    uninitialized(vm, &class)
}

fn abstract_generate(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error(
        "Error",
        "Cannot call abstract method Random\\Engine::generate()",
    ))
}

/// `Random\Engine\*::generate()`: the next output as little-endian bytes.
fn engine_generate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let r = native_generate(vm, &this)?;
    let bytes: Vec<u8> = (0..r.size).map(|i| (r.result >> (i * 8)) as u8).collect();
    Ok(Value::from(bytes))
}

fn engine_serialize_state(state: &EngineState) -> Option<PhpArray> {
    match state {
        EngineState::Mt19937(s) => Some(s.serialize()),
        EngineState::Pcg(s) => Some(s.serialize()),
        EngineState::Xoshiro(s) => Some(s.serialize()),
        EngineState::Secure => None,
    }
}

fn engine_unserialize_state(state: &mut EngineState, data: &PhpArray) -> bool {
    match state {
        EngineState::Mt19937(s) => s.unserialize(data),
        EngineState::Pcg(s) => s.unserialize(data),
        EngineState::Xoshiro(s) => s.unserialize(data),
        EngineState::Secure => false,
    }
}

fn engine_serialize_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let states = match this.native::<EngineState>() {
        Some(s) => engine_serialize_state(&s),
        None => return Err(state_missing(vm, &this)),
    };
    let Some(states) = states else {
        return Err(vm.throw_error("Exception", "Engine serialize failed"));
    };
    Ok(Value::array(PhpArray::from_values(vec![
        Value::empty_array(),
        Value::array(states),
    ])))
}

fn invalid_serialization(vm: &mut Vm, class: &str, previous: Option<ObjRef>) -> Flow {
    let ex = vm.create_exception(
        "Exception",
        format!("Invalid serialization data for {class} object"),
        0,
    );
    if let Some(p) = previous {
        ex.set_prop(b"previous", Value::Object(p));
    }
    Flow::Exception(ex)
}

/// Runs `r`, converting a thrown exception into the generic "Invalid
/// serialization data" exception (with the original as `previous`).
pub fn or_invalid_serialization<T>(vm: &mut Vm, class: &str, r: PResult<T>) -> PResult<T> {
    match r {
        Err(Flow::Exception(e)) => Err(invalid_serialization(vm, class, Some(e))),
        other => other,
    }
}

fn engine_unserialize_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let class = this.class().name.to_string();
    let data = args.array(0).clone();
    if data.len() != 2 {
        return Err(invalid_serialization(vm, &class, None));
    }
    let Some(Value::Array(members)) = data.get_int(0).map(Value::unref) else {
        return Err(invalid_serialization(vm, &class, None));
    };
    let loaded = properties_load(vm, &this, &members);
    or_invalid_serialization(vm, &class, loaded)?;
    let Some(Value::Array(states)) = data.get_int(1).map(Value::unref) else {
        return Err(invalid_serialization(vm, &class, None));
    };
    let ok = match this.native_mut::<EngineState>() {
        Some(mut s) => engine_unserialize_state(&mut s, &states),
        None => return Err(state_missing(vm, &this)),
    };
    if !ok {
        return Err(invalid_serialization(vm, &class, None));
    }
    Ok(Value::Null)
}

fn engine_debug_info_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let mut out = PhpArray::new();
    for (name, value) in this.properties() {
        out.insert(php_runtime::ArrayKey::Str(name), value);
    }
    let states = match this.native::<EngineState>() {
        Some(s) => engine_serialize_state(&s),
        None => return Err(state_missing(vm, &this)),
    };
    if let Some(states) = states {
        out.insert_str("__states", Value::array(states));
    }
    Ok(Value::array(out))
}

fn mt19937_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let mode = match args.int(1) {
        m if m == MT_RAND_MT19937 as i64 => MT_RAND_MT19937,
        m if m == MT_RAND_PHP as i64 => {
            vm.engine_deprecated("The MT_RAND_PHP variant of Mt19937 is deprecated")?;
            MT_RAND_PHP
        }
        _ => {
            return Err(vm.argument_value_error(2, "must be either MT_RAND_MT19937 or MT_RAND_PHP"));
        }
    };
    if let Some(mut st) = this.native_mut::<EngineState>()
        && let EngineState::Mt19937(s) = &mut *st
    {
        s.mode = mode;
    }
    let seed = match args.int_opt(0) {
        Some(s) => s,
        None => {
            let mut b = [0u8; 8];
            if let Err(e) = random_bytes(&mut b) {
                return Err(throw_seed_failure(vm, &e));
            }
            i64::from_le_bytes(b)
        }
    };
    match this.native_mut::<EngineState>().as_deref_mut() {
        Some(EngineState::Mt19937(s)) => s.seed32(seed as u32),
        _ => return Err(state_missing(vm, &this)),
    }
    Ok(Value::Null)
}

/// Eight-byte little-endian groups of a seed string.
fn le_words<const N: usize>(bytes: &[u8]) -> [u64; N] {
    let mut t = [0u64; N];
    for (i, w) in t.iter_mut().enumerate() {
        let mut b = [0u8; 8];
        b.copy_from_slice(&bytes[i * 8..i * 8 + 8]);
        *w = u64::from_le_bytes(b);
    }
    t
}

fn pcg_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let seed: u128 = match args.get(0) {
        Value::Null | Value::Undef => {
            let mut b = [0u8; 16];
            if let Err(e) = random_bytes(&mut b) {
                return Err(throw_seed_failure(vm, &e));
            }
            u128::from_le_bytes(b)
        }
        Value::String(s) => {
            if s.len() != 16 {
                return Err(vm.argument_value_error(1, "must be a 16 byte (128 bit) string"));
            }
            let t = le_words::<2>(s.as_bytes());
            ((t[0] as u128) << 64) | t[1] as u128
        }
        v => v.as_int().map_or(0, |i| i as u64 as u128),
    };
    match this.native_mut::<EngineState>().as_deref_mut() {
        Some(EngineState::Pcg(s)) => s.seed128(seed),
        _ => return Err(state_missing(vm, &this)),
    }
    Ok(Value::Null)
}

fn pcg_jump(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let advance = args.int(0);
    if advance < 0 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
    }
    match this.native_mut::<EngineState>().as_deref_mut() {
        Some(EngineState::Pcg(s)) => s.advance(advance as u64),
        _ => return Err(state_missing(vm, &this)),
    }
    Ok(Value::Null)
}

fn xoshiro_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let seeded: Xoshiro256 = match args.get(0) {
        Value::Null | Value::Undef => {
            let mut x = Xoshiro256::default();
            loop {
                let mut b = [0u8; 32];
                if let Err(e) = random_bytes(&mut b) {
                    return Err(throw_seed_failure(vm, &e));
                }
                let t = le_words::<4>(&b);
                if t.iter().any(|w| *w != 0) {
                    x.seed256(t[0], t[1], t[2], t[3]);
                    break x;
                }
            }
        }
        Value::String(s) => {
            if s.len() != 32 {
                return Err(vm.argument_value_error(1, "must be a 32 byte (256 bit) string"));
            }
            let t = le_words::<4>(s.as_bytes());
            if t.iter().all(|w| *w == 0) {
                return Err(vm.argument_value_error(1, "must not consist entirely of NUL bytes"));
            }
            let mut x = Xoshiro256::default();
            x.seed256(t[0], t[1], t[2], t[3]);
            x
        }
        v => {
            let mut x = Xoshiro256::default();
            x.seed64(v.as_int().unwrap_or(0) as u64);
            x
        }
    };
    match this.native_mut::<EngineState>().as_deref_mut() {
        Some(EngineState::Xoshiro(s)) => *s = seeded,
        _ => return Err(state_missing(vm, &this)),
    }
    Ok(Value::Null)
}

fn xoshiro_jump(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    match this.native_mut::<EngineState>().as_deref_mut() {
        Some(EngineState::Xoshiro(s)) => s.jump(),
        _ => return Err(state_missing(vm, &this)),
    }
    Ok(Value::Null)
}

fn xoshiro_jump_long(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    match this.native_mut::<EngineState>().as_deref_mut() {
        Some(EngineState::Xoshiro(s)) => s.jump_long(),
        _ => return Err(state_missing(vm, &this)),
    }
    Ok(Value::Null)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

static ENGINE_GENERATE_ABSTRACT: NativeFunction = NativeFunction {
    sig: "public function generate(): string",
    handler: abstract_generate,
};
static GENERATE: NativeFunction = NativeFunction {
    sig: "public function generate(): string",
    handler: engine_generate,
};
static SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array",
    handler: engine_serialize_method,
};
static UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __unserialize(array $data): void",
    handler: engine_unserialize_method,
};
static DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array",
    handler: engine_debug_info_method,
};
static MT_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(int|null $seed = null, int $mode = MT_RAND_MT19937)",
    handler: mt19937_construct,
};
static PCG_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(string|int|null $seed = null)",
    handler: pcg_construct,
};
static PCG_JUMP: NativeFunction = NativeFunction {
    sig: "public function jump(int $advance): void",
    handler: pcg_jump,
};
static XOSHIRO_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(string|int|null $seed = null)",
    handler: xoshiro_construct,
};
static XOSHIRO_JUMP: NativeFunction = NativeFunction {
    sig: "public function jump(): void",
    handler: xoshiro_jump,
};
static XOSHIRO_JUMP_LONG: NativeFunction = NativeFunction {
    sig: "public function jumpLong(): void",
    handler: xoshiro_jump_long,
};

fn add_methods(ce: &mut ClassEntry, class: &str, methods: &[&'static NativeFunction]) {
    for nf in methods {
        ce.add_method(Function::native_method(nf, class, member_flags::PUBLIC));
    }
}

/// Registers the interfaces, the exception classes and the engine classes.
pub fn register(r: &mut Registry<'_>) {
    // Random\Engine
    let mut engine = ClassEntry::new(ENGINE_INTERFACE, class_flags::INTERFACE);
    engine.add_method(Function::native_method(
        &ENGINE_GENERATE_ABSTRACT,
        ENGINE_INTERFACE,
        member_flags::PUBLIC | member_flags::ABSTRACT,
    ));
    let engine = r.class(engine);

    // Random\CryptoSafeEngine
    let mut crypto = ClassEntry::new(CRYPTO_SAFE_ENGINE_INTERFACE, class_flags::INTERFACE);
    crypto.interfaces.push(engine.clone());
    let crypto = r.class(crypto);

    // Random\RandomError, Random\BrokenRandomEngineError, Random\RandomException
    let error = r.find_class("Error").expect("Error is a core class");
    let random_error = r.class(ClassEntry::new_subclass(RANDOM_ERROR, 0, &error));
    r.class(ClassEntry::new_subclass(
        BROKEN_ENGINE_ERROR,
        0,
        &random_error,
    ));
    let exception = r
        .find_class("Exception")
        .expect("Exception is a core class");
    r.class(ClassEntry::new_subclass(RANDOM_EXCEPTION, 0, &exception));

    // Random\Engine\Mt19937
    let mut ce = ClassEntry::new(MT19937_CLASS, class_flags::FINAL);
    ce.interfaces.push(engine.clone());
    add_methods(
        &mut ce,
        MT19937_CLASS,
        &[
            &MT_CONSTRUCT,
            &GENERATE,
            &SERIALIZE,
            &UNSERIALIZE,
            &DEBUG_INFO,
        ],
    );
    ce.handlers.create_object = Some(create_mt19937);
    ce.handlers.clone_obj = Some(clone_engine);
    r.class(ce);

    // Random\Engine\PcgOneseq128XslRr64
    let mut ce = ClassEntry::new(PCG_CLASS, class_flags::FINAL);
    ce.interfaces.push(engine.clone());
    add_methods(
        &mut ce,
        PCG_CLASS,
        &[
            &PCG_CONSTRUCT,
            &GENERATE,
            &PCG_JUMP,
            &SERIALIZE,
            &UNSERIALIZE,
            &DEBUG_INFO,
        ],
    );
    ce.handlers.create_object = Some(create_pcg);
    ce.handlers.clone_obj = Some(clone_engine);
    r.class(ce);

    // Random\Engine\Xoshiro256StarStar
    let mut ce = ClassEntry::new(XOSHIRO_CLASS, class_flags::FINAL);
    ce.interfaces.push(engine.clone());
    add_methods(
        &mut ce,
        XOSHIRO_CLASS,
        &[
            &XOSHIRO_CONSTRUCT,
            &GENERATE,
            &XOSHIRO_JUMP,
            &XOSHIRO_JUMP_LONG,
            &SERIALIZE,
            &UNSERIALIZE,
            &DEBUG_INFO,
        ],
    );
    ce.handlers.create_object = Some(create_xoshiro);
    ce.handlers.clone_obj = Some(clone_engine);
    r.class(ce);

    // Random\Engine\Secure
    let mut ce = ClassEntry::new(
        SECURE_CLASS,
        class_flags::FINAL | class_flags::NOT_SERIALIZABLE,
    );
    ce.interfaces.push(crypto);
    ce.interfaces.push(engine);
    add_methods(&mut ce, SECURE_CLASS, &[&GENERATE]);
    ce.handlers.create_object = Some(create_secure);
    ce.handlers.clone_obj = Some(clone_forbidden);
    r.class(ce);
}

/// The class entry of `name`, for `instantiate`.
pub fn find_class(vm: &Vm, name: &str) -> ClassRef {
    vm.find_class(name.as_bytes())
        .unwrap_or_else(|| panic!("class {name} is registered by ext/random"))
}
