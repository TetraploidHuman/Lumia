//! Value emission — arithmetic and binary/unary ops.

mod checked;
mod ops;

#[cfg(test)]
mod tests {
    use super::super::super::Codegen;
    use lumia_ty::Type;

    #[test]
    fn bit_identity_scalars_are_int_bool_unit_only() {
        assert!(Codegen::is_bit_identity_scalar(&Type::Int));
        assert!(Codegen::is_bit_identity_scalar(&Type::Bool));
        assert!(Codegen::is_bit_identity_scalar(&Type::Unit));
        assert!(!Codegen::is_bit_identity_scalar(&Type::Float));
        assert!(!Codegen::is_bit_identity_scalar(&Type::String));
        assert!(!Codegen::is_bit_identity_scalar(&Type::Char));
        assert!(!Codegen::is_bit_identity_scalar(&Type::List(Box::new(Type::Int))));
        assert!(!Codegen::is_bit_identity_scalar(&Type::Var(0)));
    }

    #[test]
    fn adt_method_name_requires_matching_named_adts() {
        let point = Type::Adt {
            name: "Point".into(),
            params: vec![Type::Int, Type::Int],
        };
        let other = Type::Adt {
            name: "Point".into(),
            params: vec![Type::Float, Type::Float],
        };
        let miss = Type::Adt {
            name: "Rect".into(),
            params: vec![],
        };
        assert_eq!(
            Codegen::adt_method_name(&point, &other).as_deref(),
            Some("Point")
        );
        assert_eq!(Codegen::adt_method_name(&point, &miss), None);
        assert_eq!(Codegen::adt_method_name(&Type::Int, &Type::Int), None);
        assert_eq!(Codegen::adt_method_name(&point, &Type::Int), None);
    }
}
