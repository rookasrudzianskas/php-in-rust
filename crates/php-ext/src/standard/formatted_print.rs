//! ext/standard/formatted_print.c — sprintf family.

use php_runtime::{PResult, PhpStr, Registry, Value, Vm};

/// `php_formatted_print`: format `args` according to `format` exactly like
/// `sprintf`. `format_arg_num` is the 1-based position of the format
/// argument in the calling PHP function (1 for sprintf, 2 for fprintf), used
/// in error messages. The string-functions work package owns this module and
/// must keep this signature.
pub fn php_formatted_print(
    vm: &mut Vm,
    format: &PhpStr,
    args: &[Value],
    format_arg_num: usize,
) -> PResult<Vec<u8>> {
    let _ = (vm, format, args, format_arg_num);
    todo!("implemented by the string functions work package")
}

pub fn register(_r: &mut Registry) {}
