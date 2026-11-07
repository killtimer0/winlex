use super::*;

fn decode(bytes: &[u8]) -> Type {
    let mut file = writer::File::new("PointerQualifiers");
    file.TypeRef("System.Runtime.CompilerServices", "IsConst");
    let index = Index::new(vec![File::new(file.into_stream()).unwrap()]);
    let mut blob = Blob::new(&index, 0, bytes);
    let ty = blob.read_type_signature(&[]);
    assert_eq!(blob.len(), 0);
    ty
}

#[test]
fn pointer_qualifiers_stay_at_their_signature_level() {
    // 0x05 is TypeRef row 1, the IsConst reference in this fixture.
    assert_eq!(
        decode(&[0x0f, 0x1f, 0x05, 0x0f, 0x08]),
        Type::PtrMut(Box::new(Type::PtrConst(Box::new(Type::I32), 1)), 1)
    );
    assert_eq!(
        decode(&[0x20, 0x05, 0x0f, 0x0f, 0x08]),
        Type::PtrConst(Box::new(Type::PtrMut(Box::new(Type::I32), 1)), 1)
    );
    assert_eq!(
        decode(&[0x1f, 0x05, 0x0f, 0x20, 0x05, 0x0f, 0x08]),
        Type::PtrConst(Box::new(Type::I32), 2)
    );
}

#[test]
fn byref_and_array_nesting_does_not_move_constness() {
    assert_eq!(
        decode(&[0x20, 0x05, 0x10, 0x08]),
        Type::RefConst(Box::new(Type::I32))
    );
    assert_eq!(
        decode(&[0x10, 0x1d, 0x0f, 0x1f, 0x05, 0x0f, 0x08]),
        Type::RefMut(Box::new(Type::Array(Box::new(Type::PtrMut(
            Box::new(Type::PtrConst(Box::new(Type::I32), 1)),
            1,
        )))))
    );
}
