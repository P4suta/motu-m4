use super::{validate_entry, Budget, MemoryLayout};

#[kani::proof]
#[kani::unwind(1)]
fn memory_layout_cannot_overflow_or_allocate_unbounded_memory() {
    let base: u32 = kani::any();
    let bytes: u32 = kani::any();
    let expected = base & 3 == 0
        && bytes >= 8
        && bytes & 3 == 0
        && bytes <= 1_048_576
        && base.checked_add(bytes).is_some()
        && !(u64::from(base) <= 0xffff_ff00 && u64::from(base) + u64::from(bytes) > 0xffff_ff00);
    assert_eq!(MemoryLayout::new(base, bytes).is_ok(), expected);
    kani::cover!(expected);
    kani::cover!(!expected);
}

#[kani::proof]
#[kani::unwind(1)]
fn instruction_budget_is_positive_and_bounded() {
    let instructions: u32 = kani::any();
    assert_eq!(
        Budget::new(instructions).is_ok(),
        instructions >= 1 && instructions <= 10_000_000
    );
    kani::cover!(Budget::new(instructions).is_ok());
    kani::cover!(Budget::new(instructions).is_err());
}

#[kani::proof]
#[kani::unwind(1)]
fn accepted_entries_are_thumb_instructions_inside_the_payload() {
    let base: u32 = kani::any();
    let bytes: u32 = kani::any();
    let entry: u32 = kani::any();
    kani::assume(bytes >= 8);
    kani::assume(base.checked_add(bytes).is_some());
    let result = validate_entry(base, bytes, entry);
    let address = entry & !1;
    let expected = entry & 1 == 1
        && address >= base
        && u64::from(address) >= u64::from(base) + 8
        && u64::from(address) + 2 <= u64::from(base) + u64::from(bytes);
    assert_eq!(result.is_ok(), expected);
    kani::cover!(expected);
    kani::cover!(!expected);
}
