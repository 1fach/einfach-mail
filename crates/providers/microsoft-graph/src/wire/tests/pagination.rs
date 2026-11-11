

#[test]
fn pagination_budget_accepts_limits_and_rejects_additional_work() {
    let mut budget = PaginationBudget::default();
    for _ in 0..MAX_PAGINATION_PAGES {
        budget
            .begin_page()
            .expect("page is within the configured limit");
    }
    assert_eq!(
        budget.begin_page(),
        Err(GraphError::PaginationLimitExceeded)
    );

    let mut byte_budget = PaginationBudget::default();
    byte_budget
        .add_response_bytes(MAX_PAGINATION_RESPONSE_BYTES)
        .expect("response bytes are within the configured limit");
    assert_eq!(
        byte_budget.add_response_bytes(1),
        Err(GraphError::PaginationLimitExceeded)
    );
}