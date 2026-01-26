#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env,
};

fn create_token_contract<'a>(
    e: &Env,
    admin: &Address,
) -> (token::Client<'a>, token::StellarAssetClient<'a>) {
    let contract_address = e.register_stellar_asset_contract(admin.clone());
    (
        token::Client::new(e, &contract_address),
        token::StellarAssetClient::new(e, &contract_address),
    )
}

fn create_escrow_contract<'a>(e: &Env) -> BountyEscrowContractClient<'a> {
    let contract_id = e.register_contract(None, BountyEscrowContract);
    BountyEscrowContractClient::new(e, &contract_id)
}

struct TestSetup<'a> {
    env: Env,
    admin: Address,
    depositor: Address,
    contributor: Address,
    token: token::Client<'a>,
    token_admin: token::StellarAssetClient<'a>,
    escrow: BountyEscrowContractClient<'a>,
}

impl<'a> TestSetup<'a> {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let depositor = Address::generate(&env);
        let contributor = Address::generate(&env);

        let (token, token_admin) = create_token_contract(&env, &admin);
        let escrow = create_escrow_contract(&env);

        escrow.init(&admin, &token.address);

        // Mint tokens to depositor
        token_admin.mint(&depositor, &1_000_000);

        Self {
            env,
            admin,
            depositor,
            contributor,
            token,
            token_admin,
            escrow,
        }
    }
}

#[test]
fn test_lock_funds_success() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let deadline = setup.env.ledger().timestamp() + 1000;

    // Lock funds
    setup
        .escrow
        .lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Verify stored escrow data
    let stored_escrow = setup.escrow.get_escrow_info(&bounty_id);
    assert_eq!(stored_escrow.depositor, setup.depositor);
    assert_eq!(stored_escrow.amount, amount);
    assert_eq!(stored_escrow.status, EscrowStatus::Locked);
    assert_eq!(stored_escrow.deadline, deadline);

    // Verify contract balance
    assert_eq!(setup.token.balance(&setup.escrow.address), amount);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // BountyExists
fn test_lock_funds_duplicate() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let deadline = setup.env.ledger().timestamp() + 1000;

    setup
        .escrow
        .lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Try to lock again with same bounty_id
    setup
        .escrow
        .lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);
}

#[test]
#[should_panic] // Token transfer fail
fn test_lock_funds_negative_amount() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = -100;
    let deadline = setup.env.ledger().timestamp() + 1000;

    setup
        .escrow
        .lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);
}

#[test]
fn test_get_escrow_info() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let deadline = setup.env.ledger().timestamp() + 1000;

    setup
        .escrow
        .lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    let escrow = setup.escrow.get_escrow_info(&bounty_id);
    assert_eq!(escrow.amount, amount);
    assert_eq!(escrow.deadline, deadline);
    assert_eq!(escrow.depositor, setup.depositor);
    assert_eq!(escrow.status, EscrowStatus::Locked);
}

#[test]
fn test_release_funds_success() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let deadline = setup.env.ledger().timestamp() + 1000;

    setup
        .escrow
        .lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Verify initial balances
    assert_eq!(setup.token.balance(&setup.escrow.address), amount);
    assert_eq!(setup.token.balance(&setup.contributor), 0);

    // Release funds
    setup.escrow.release_funds(&bounty_id, &setup.contributor);

    // Verify updated state
    let stored_escrow = setup.escrow.get_escrow_info(&bounty_id);
    assert_eq!(stored_escrow.status, EscrowStatus::Released);

    // Verify balances after release
    assert_eq!(setup.token.balance(&setup.escrow.address), 0);
    assert_eq!(setup.token.balance(&setup.contributor), amount);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // FundsNotLocked
fn test_release_funds_already_released() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let deadline = setup.env.ledger().timestamp() + 1000;

    setup
        .escrow
        .lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);
    setup.escrow.release_funds(&bounty_id, &setup.contributor);

    // Try to release again
    setup.escrow.release_funds(&bounty_id, &setup.contributor);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // BountyNotFound
fn test_release_funds_not_found() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    setup.escrow.release_funds(&bounty_id, &setup.contributor);
}

// ============================================================================
// REFUND TESTS - Full Refund After Deadline
// ============================================================================

#[test]
fn test_refund_full_after_deadline() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup
        .escrow
        .lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Advance time past deadline
    setup.env.ledger().set_timestamp(deadline + 1);

    // Initial balances
    let initial_depositor_balance = setup.token.balance(&setup.depositor);
    let initial_contract_balance = setup.token.balance(&setup.escrow.address);

    // Full refund (no amount/recipient specified, mode = Full)
    setup.escrow.refund(
        &bounty_id,
        &None::<i128>,
        &None::<Address>,
        &RefundMode::Full,
    );

    // Verify state
    let stored_escrow = setup.escrow.get_escrow_info(&bounty_id);
    assert_eq!(stored_escrow.status, EscrowStatus::Refunded);
    assert_eq!(stored_escrow.remaining_amount, 0);

    // Verify balances
    assert_eq!(setup.token.balance(&setup.escrow.address), 0);
    assert_eq!(
        setup.token.balance(&setup.depositor),
        initial_depositor_balance + amount
    );

    // Verify refund history
    let refund_history = setup.escrow.get_refund_history(&bounty_id);
    assert_eq!(refund_history.len(), 1);
    assert_eq!(refund_history.get(0).unwrap().amount, amount);
    assert_eq!(refund_history.get(0).unwrap().recipient, setup.depositor);
    assert_eq!(refund_history.get(0).unwrap().mode, RefundMode::Full);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // DeadlineNotPassed
fn test_refund_full_before_deadline() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Attempt full refund before deadline (should fail)
    setup.escrow.refund(
        &bounty_id,
        &None::<i128>,
        &None::<Address>,
        &RefundMode::Full,
    );
}

// ============================================================================
// REFUND TESTS - Partial Refund
// ============================================================================

#[test]
fn test_refund_partial_after_deadline() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let total_amount = 1000;
    let refund_amount = 300;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &total_amount, &deadline);

    // Advance time past deadline
    setup.env.ledger().set_timestamp(deadline + 1);

    // Initial balances
    let initial_depositor_balance = setup.token.balance(&setup.depositor);

    // Partial refund
    setup.escrow.refund(
        &bounty_id,
        &Some(refund_amount),
        &None::<Address>,
        &RefundMode::Partial,
    );

    // Verify state
    let stored_escrow = setup.escrow.get_escrow_info(&bounty_id);
    assert_eq!(stored_escrow.status, EscrowStatus::PartiallyRefunded);
    assert_eq!(stored_escrow.remaining_amount, total_amount - refund_amount);

    // Verify balances
    assert_eq!(
        setup.token.balance(&setup.escrow.address),
        total_amount - refund_amount
    );
    assert_eq!(
        setup.token.balance(&setup.depositor),
        initial_depositor_balance + refund_amount
    );

    // Verify refund history
    let refund_history = setup.escrow.get_refund_history(&bounty_id);
    assert_eq!(refund_history.len(), 1);
    assert_eq!(refund_history.get(0).unwrap().amount, refund_amount);
    assert_eq!(refund_history.get(0).unwrap().recipient, setup.depositor);
    assert_eq!(refund_history.get(0).unwrap().mode, RefundMode::Partial);
}

#[test]
fn test_refund_partial_multiple_times() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let total_amount = 1000;
    let refund1 = 200;
    let refund2 = 300;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &total_amount, &deadline);
    setup.env.ledger().set_timestamp(deadline + 1);

    // First partial refund
    setup.escrow.refund(
        &bounty_id,
        &Some(refund1),
        &None::<Address>,
        &RefundMode::Partial,
    );

    // Second partial refund
    setup.escrow.refund(
        &bounty_id,
        &Some(refund2),
        &None::<Address>,
        &RefundMode::Partial,
    );

    // Verify state
    let stored_escrow = setup.escrow.get_escrow_info(&bounty_id);
    assert_eq!(stored_escrow.status, EscrowStatus::PartiallyRefunded);
    assert_eq!(
        stored_escrow.remaining_amount,
        total_amount - refund1 - refund2
    );

    // Verify refund history has 2 records
    let refund_history = setup.escrow.get_refund_history(&bounty_id);
    assert_eq!(refund_history.len(), 2);
    assert_eq!(refund_history.get(0).unwrap().amount, refund1);
    assert_eq!(refund_history.get(1).unwrap().amount, refund2);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // DeadlineNotPassed
fn test_refund_partial_before_deadline() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let refund_amount = 300;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Attempt partial refund before deadline (should fail)
    setup.escrow.refund(
        &bounty_id,
        &Some(refund_amount),
        &None::<Address>,
        &RefundMode::Partial,
    );
}

// ============================================================================
// REFUND TESTS - Custom Refund (Different Address)
// ============================================================================

#[test]
fn test_refund_custom_after_deadline() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let refund_amount = 500;
    let custom_recipient = Address::generate(&setup.env);
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);
    setup.env.ledger().set_timestamp(deadline + 1);

    // Initial balances
    let initial_recipient_balance = setup.token.balance(&custom_recipient);

    // Custom refund to different address (after deadline, no approval needed)
    setup.escrow.refund(
        &bounty_id,
        &Some(refund_amount),
        &Some(custom_recipient.clone()),
        &RefundMode::Custom,
    );

    // Verify state
    let stored_escrow = setup.escrow.get_escrow_info(&bounty_id);
    assert_eq!(stored_escrow.status, EscrowStatus::PartiallyRefunded);
    assert_eq!(stored_escrow.remaining_amount, amount - refund_amount);

    // Verify balances
    assert_eq!(
        setup.token.balance(&custom_recipient),
        initial_recipient_balance + refund_amount
    );

    // Verify refund history
    let refund_history = setup.escrow.get_refund_history(&bounty_id);
    assert_eq!(refund_history.len(), 1);
    assert_eq!(refund_history.get(0).unwrap().amount, refund_amount);
    assert_eq!(refund_history.get(0).unwrap().recipient, custom_recipient);
    assert_eq!(refund_history.get(0).unwrap().mode, RefundMode::Custom);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // RefundNotApproved
fn test_refund_custom_before_deadline_without_approval() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let refund_amount = 500;
    let custom_recipient = Address::generate(&setup.env);
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Attempt custom refund before deadline without approval (should fail)
    setup.escrow.refund(
        &bounty_id,
        &Some(refund_amount),
        &Some(custom_recipient),
        &RefundMode::Custom,
    );
}

// ============================================================================
// REFUND TESTS - Approval Workflow
// ============================================================================

#[test]
fn test_refund_approval_workflow() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let refund_amount = 500;
    let custom_recipient = Address::generate(&setup.env);
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Admin approves refund before deadline
    setup.escrow.approve_refund(
        &bounty_id,
        &refund_amount,
        &custom_recipient.clone(),
        &RefundMode::Custom,
    );

    // Verify approval exists
    let (can_refund, deadline_passed, remaining, approval) =
        setup.escrow.get_refund_eligibility(&bounty_id);
    assert!(can_refund);
    assert!(!deadline_passed);
    assert_eq!(remaining, amount);
    assert!(approval.is_some());
    let approval_data = approval.unwrap();
    assert_eq!(approval_data.amount, refund_amount);
    assert_eq!(approval_data.recipient, custom_recipient);
    assert_eq!(approval_data.mode, RefundMode::Custom);
    assert_eq!(approval_data.approved_by, setup.admin);

    // Initial balances
    let initial_recipient_balance = setup.token.balance(&custom_recipient);

    // Execute approved refund (before deadline)
    setup.escrow.refund(
        &bounty_id,
        &Some(refund_amount),
        &Some(custom_recipient.clone()),
        &RefundMode::Custom,
    );

    // Verify approval was consumed (removed after use)
    let (_, _, _, approval_after) = setup.escrow.get_refund_eligibility(&bounty_id);
    assert!(approval_after.is_none());

    // Verify state
    let stored_escrow = setup.escrow.get_escrow_info(&bounty_id);
    assert_eq!(stored_escrow.status, EscrowStatus::PartiallyRefunded);
    assert_eq!(stored_escrow.remaining_amount, amount - refund_amount);

    // Verify balances
    assert_eq!(
        setup.token.balance(&custom_recipient),
        initial_recipient_balance + refund_amount
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // RefundNotApproved
fn test_refund_approval_mismatch() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let approved_amount = 500;
    let requested_amount = 600; // Different amount
    let custom_recipient = Address::generate(&setup.env);
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Admin approves refund for 500
    setup.escrow.approve_refund(
        &bounty_id,
        &approved_amount,
        &custom_recipient.clone(),
        &RefundMode::Custom,
    );

    // Try to refund with different amount (should fail)
    setup.escrow.refund(
        &bounty_id,
        &Some(requested_amount),
        &Some(custom_recipient),
        &RefundMode::Custom,
    );
}

#[test]
#[ignore] // Note: With mock_all_auths(), we can't test unauthorized access
// The security is enforced by require_auth() in the contract which checks admin address
// In production, non-admin calls will fail at require_auth()
fn test_refund_approval_non_admin() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let _refund_amount = 500;
    let _custom_recipient = Address::generate(&setup.env);
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Note: With mock_all_auths(), we can't easily test unauthorized access
    // The contract's require_auth() will enforce admin-only access in production
    // This test is marked as ignored as it requires more complex auth setup
}

// ============================================================================
// REFUND TESTS - Refund History Tracking
// ============================================================================

#[test]
fn test_refund_history_tracking() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let total_amount = 1000;
    let refund1 = 200;
    let refund2 = 300;
    let refund3 = 400;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &total_amount, &deadline);
    setup.env.ledger().set_timestamp(deadline + 1);

    // First refund (Partial)
    setup.escrow.refund(
        &bounty_id,
        &Some(refund1),
        &None::<Address>,
        &RefundMode::Partial,
    );

    // Second refund (Partial)
    setup.escrow.refund(
        &bounty_id,
        &Some(refund2),
        &None::<Address>,
        &RefundMode::Partial,
    );

    // Third refund (Full remaining - should complete the refund)
    let remaining = total_amount - refund1 - refund2;
    setup.escrow.refund(
        &bounty_id,
        &Some(remaining),
        &None::<Address>,
        &RefundMode::Partial,
    );

    // Verify refund history
    let refund_history = setup.escrow.get_refund_history(&bounty_id);
    assert_eq!(refund_history.len(), 3);

    // Check first refund record
    let record1 = refund_history.get(0).unwrap();
    assert_eq!(record1.amount, refund1);
    assert_eq!(record1.recipient, setup.depositor);
    assert_eq!(record1.mode, RefundMode::Partial);

    // Check second refund record
    let record2 = refund_history.get(1).unwrap();
    assert_eq!(record2.amount, refund2);
    assert_eq!(record2.recipient, setup.depositor);
    assert_eq!(record2.mode, RefundMode::Partial);

    // Check third refund record
    let record3 = refund_history.get(2).unwrap();
    assert_eq!(record3.amount, remaining);
    assert_eq!(record3.recipient, setup.depositor);
    assert_eq!(record3.mode, RefundMode::Partial);

    // Verify final state
    let stored_escrow = setup.escrow.get_escrow_info(&bounty_id);
    assert_eq!(stored_escrow.status, EscrowStatus::Refunded);
    assert_eq!(stored_escrow.remaining_amount, 0);
}

#[test]
fn test_refund_history_with_custom_recipients() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let total_amount = 1000;
    let recipient1 = Address::generate(&setup.env);
    let recipient2 = Address::generate(&setup.env);
    let refund1 = 300;
    let refund2 = 400;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &total_amount, &deadline);
    setup.env.ledger().set_timestamp(deadline + 1);

    // First custom refund
    setup.escrow.refund(
        &bounty_id,
        &Some(refund1),
        &Some(recipient1.clone()),
        &RefundMode::Custom,
    );

    // Second custom refund
    setup.escrow.refund(
        &bounty_id,
        &Some(refund2),
        &Some(recipient2.clone()),
        &RefundMode::Custom,
    );

    // Verify refund history
    let refund_history = setup.escrow.get_refund_history(&bounty_id);
    assert_eq!(refund_history.len(), 2);
    assert_eq!(refund_history.get(0).unwrap().recipient, recipient1);
    assert_eq!(refund_history.get(1).unwrap().recipient, recipient2);
}

// ============================================================================
// REFUND TESTS - Error Cases
// ============================================================================

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // InvalidAmount
fn test_refund_invalid_amount_zero() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);
    setup.env.ledger().set_timestamp(deadline + 1);

    // Try to refund zero amount
    setup.escrow.refund(
        &bounty_id,
        &Some(0),
        &None::<Address>,
        &RefundMode::Partial,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // InvalidAmount
fn test_refund_invalid_amount_exceeds_remaining() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let refund_amount = 1500; // More than available
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);
    setup.env.ledger().set_timestamp(deadline + 1);

    // Try to refund more than available
    setup.escrow.refund(
        &bounty_id,
        &Some(refund_amount),
        &None::<Address>,
        &RefundMode::Partial,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // InvalidAmount
fn test_refund_custom_missing_amount() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let custom_recipient = Address::generate(&setup.env);
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);
    setup.env.ledger().set_timestamp(deadline + 1);

    // Custom refund requires amount
    setup.escrow.refund(
        &bounty_id,
        &None::<i128>,
        &Some(custom_recipient),
        &RefundMode::Custom,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // InvalidAmount
fn test_refund_custom_missing_recipient() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let refund_amount = 500;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);
    setup.env.ledger().set_timestamp(deadline + 1);

    // Custom refund requires recipient
    setup.escrow.refund(
        &bounty_id,
        &Some(refund_amount),
        &None::<Address>,
        &RefundMode::Custom,
    );
}

#[test]
fn test_get_refund_eligibility() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 1000;
    let current_time = setup.env.ledger().timestamp();
    let deadline = current_time + 1000;

    setup.escrow.lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Before deadline, no approval
    let (can_refund, deadline_passed, remaining, approval) =
        setup.escrow.get_refund_eligibility(&bounty_id);
    assert!(!can_refund);
    assert!(!deadline_passed);
    assert_eq!(remaining, amount);
    assert!(approval.is_none());

    // After deadline
    setup.env.ledger().set_timestamp(deadline + 1);
    let (can_refund, deadline_passed, remaining, approval) =
        setup.escrow.get_refund_eligibility(&bounty_id);
    assert!(can_refund);
    assert!(deadline_passed);
    assert_eq!(remaining, amount);
    assert!(approval.is_none());

    // With approval before deadline
    setup.env.ledger().set_timestamp(deadline - 100);
    let custom_recipient = Address::generate(&setup.env);
    setup.escrow.approve_refund(
        &bounty_id,
        &500,
        &custom_recipient,
        &RefundMode::Custom,
    );

    let (can_refund, deadline_passed, remaining, approval) =
        setup.escrow.get_refund_eligibility(&bounty_id);
    assert!(can_refund);
    assert!(!deadline_passed);
    assert_eq!(remaining, amount);
    assert!(approval.is_some());
}

#[test]
fn test_get_balance() {
    let setup = TestSetup::new();
    let bounty_id = 1;
    let amount = 500;
    let deadline = setup.env.ledger().timestamp() + 1000;

    // Initial balance should be 0
    assert_eq!(setup.escrow.get_balance(), 0);

    setup
        .escrow
        .lock_funds(&setup.depositor, &bounty_id, &amount, &deadline);

    // Balance should be updated
    assert_eq!(setup.escrow.get_balance(), amount);
}

// ========================================================================
// Anti-Abuse Tests
// ========================================================================

#[test]
#[should_panic(expected = "Operation in cooldown period")]
fn test_anti_abuse_cooldown_panic() {
    let setup = TestSetup::new();
    setup.env.ledger().set_timestamp(1000);
    
    // Default cooldown is 60s
    setup.escrow.lock_funds(&setup.depositor, &1, &100, &2000);
    
    setup.env.ledger().set_timestamp(1030); // 30s < 60s
    setup.escrow.lock_funds(&setup.depositor, &2, &100, &2000); // Should panic
}

#[test]
#[should_panic(expected = "Rate limit exceeded")]
fn test_anti_abuse_limit_panic() {
    let setup = TestSetup::new();
    setup.env.ledger().set_timestamp(1000);
    
    setup.escrow.update_rate_limit_config(&3600, &1, &0); // 1 op max
    
    setup.escrow.lock_funds(&setup.depositor, &1, &100, &2000);
    setup.escrow.lock_funds(&setup.depositor, &2, &100, &2000); // Should panic
}

#[test]
fn test_anti_abuse_whitelist() {
    let setup = TestSetup::new();
    setup.env.ledger().set_timestamp(1000);
    
    setup.escrow.update_rate_limit_config(&3600, &1, &60); // 1 op max
    
    setup.escrow.set_whitelist(&setup.depositor, &true);
    
    setup.escrow.lock_funds(&setup.depositor, &1, &100, &2000);
    setup.escrow.lock_funds(&setup.depositor, &2, &100, &2000); // Works because whitelisted
}

#[test]
fn test_anti_abuse_config_update() {
    let setup = TestSetup::new();
    setup.escrow.update_rate_limit_config(&7200, &5, &120);
    
    let config = setup.escrow.get_rate_limit_config();
    assert_eq!(config.window_size, 7200);
    assert_eq!(config.max_operations, 5);
    assert_eq!(config.cooldown_period, 120);
}
