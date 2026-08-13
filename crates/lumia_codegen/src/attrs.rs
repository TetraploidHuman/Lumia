//! LLVM function attributes for middle-end cooperation.

use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::context::Context;
use inkwell::values::FunctionValue;

fn enum_attr(context: &Context, name: &str) -> Attribute {
    let kind = Attribute::get_named_enum_kind_id(name);
    context.create_enum_attribute(kind, 0)
}

/// Lumia never unwinds into LLVM: traps abort / `#[cfg(test)]` panic stays in RT tests.
pub(crate) fn add_nounwind(context: &Context, fv: FunctionValue<'_>) {
    fv.add_attribute(AttributeLoc::Function, enum_attr(context, "nounwind"));
}
