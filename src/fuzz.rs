use crate::input::*;

use crate::config::*;
use crate::DAY_IN_LEDGERS;
use arbitrary::Unstructured;
use itertools::Itertools;
use libfuzzer_sys::{fuzz_target, Corpus};
use num_bigint::BigInt;
use soroban_ledger_snapshot::LedgerSnapshot;
use soroban_sdk::testutils::{
    arbitrary::{arbitrary, SorobanArbitrary},
    Address as _, AuthorizedFunction, AuthorizedInvocation, Ledger, LedgerInfo, Logs, MockAuth,
    MockAuthInvoke,
};
use soroban_sdk::{
    token::{Client, StellarAssetClient},
    Address, Bytes, Env, Error, FromVal, IntoVal, InvokeError, String, TryFromVal, Val,
};
use std::collections::BTreeMap;
use std::vec::Vec as RustVec;

type TokenContractResult =
    Result<Result<(), <() as TryFromVal<Env, Val>>::Error>, Result<Error, InvokeError>>;

pub fn fuzz_token(config: Config, input: Input) -> Corpus {
    if input.commands.is_empty() {
        return Corpus::Reject;
    }

    //eprintln!("input: {input:#?}");
    let mut env = create_env();

    let token_contract_id_bytes: RustVec<u8>;

    // Do initial setup, including registering the contract.
    {
        let admin = Address::from_val(&env, &input.addresses[0]);
        let accounts = input
            .addresses
            .iter()
            .map(|a| Address::from_val(&env, a))
            .collect::<Vec<_>>();

        if !require_unique_addresses(&accounts) {
            return Corpus::Reject;
        }

        if !require_contract_addresses(&accounts) {
            return Corpus::Reject;
        }

        let token_contract_id = config.register_contract_init(&env, &admin);
        token_contract_id_bytes = address_to_bytes(&token_contract_id);
    }

    let mut contract_state = ContractState::init(&env);
    let mut current_state =
        CurrentState::new(&config, &env, &input.addresses, &token_contract_id_bytes);

    let mut results: Vec<(&'static str, bool)> = vec![];

    let mut log_result = |name, r: &Result<_, _>| {
        results.push((name, r.is_ok()));
    };

    for command in input.commands {
        // The Env may be different for each step, so we need to reconstruct
        // everything that depends on it.
        env.mock_all_auths();
        env.budget().reset_unlimited();

        let admin_client = &current_state.admin_client;
        let token_client = &current_state.token_client;
        let accounts = &current_state.accounts;

        contract_state.name = string_to_bytes(token_client.name());
        contract_state.symbol = string_to_bytes(token_client.symbol());
        contract_state.decimals = token_client.decimals();

        // println!("------- command: {:#?}\n--------", command);
        match command {
            Command::Mint(input) => {
                let r = admin_client.try_mint(&accounts[input.to_account_index], &input.amount);

                log_result("mint", &r);
                if input.amount < 0 {
                    assert!(r.is_err());
                }

                verify_token_contract_result(&env, &r);

                if let Ok(r) = r {
                    let r = r.unwrap();

                    contract_state.add_balance(&accounts[input.to_account_index], input.amount);

                    contract_state.sum_of_mints =
                        contract_state.sum_of_mints + BigInt::from(input.amount);
                }
            }
            Command::Approve(input) => {
                let r = token_client.try_approve(
                    &accounts[input.from_account_index],
                    &accounts[input.spender_account_index],
                    &input.amount,
                    &input.expiration_ledger,
                );

                log_result("approve", &r);

                if input.amount < 0 {
                    assert!(r.is_err());
                }

                verify_token_contract_result(&env, &r);

                if let Ok(r) = r {
                    let r = r.unwrap();

                    contract_state.set_allowance(
                        &accounts[input.from_account_index],
                        &accounts[input.spender_account_index],
                        input.amount,
                    );
                }
            }
            Command::TransferFrom(input) => {
                let r = token_client.try_transfer_from(
                    &accounts[input.spender_account_index],
                    &accounts[input.from_account_index],
                    &accounts[input.to_account_index],
                    &input.amount,
                );

                log_result("transfer_from", &r);

                if input.amount < 0 {
                    assert!(r.is_err());
                }

                verify_token_contract_result(&env, &r);

                if let Ok(r) = r {
                    let r = r.unwrap();

                    contract_state.sub_balance(&accounts[input.from_account_index], input.amount);
                    contract_state.add_balance(&accounts[input.to_account_index], input.amount);

                    contract_state.sub_allowance(
                        &accounts[input.from_account_index],
                        &accounts[input.spender_account_index],
                        input.amount,
                    );
                }
            }
            Command::Transfer(input) => {
                let r = token_client.try_transfer(
                    &accounts[input.from_account_index],
                    &accounts[input.to_account_index],
                    &input.amount,
                );

                log_result("transfer", &r);

                if input.amount < 0 {
                    assert!(r.is_err());
                }

                verify_token_contract_result(&env, &r);

                if let Ok(r) = r {
                    let r = r.unwrap();

                    contract_state.sub_balance(&accounts[input.from_account_index], input.amount);
                    contract_state.add_balance(&accounts[input.to_account_index], input.amount);
                }
            }
            Command::BurnFrom(input) => {
                let r = token_client.try_burn_from(
                    &accounts[input.spender_account_index],
                    &accounts[input.from_account_index],
                    &input.amount,
                );

                log_result("burn_from", &r);

                if input.amount < 0 {
                    assert!(r.is_err());
                }

                verify_token_contract_result(&env, &r);

                if let Ok(r) = r {
                    let r = r.unwrap();

                    contract_state.sub_balance(&accounts[input.from_account_index], input.amount);

                    contract_state.sub_allowance(
                        &accounts[input.from_account_index],
                        &accounts[input.spender_account_index],
                        input.amount,
                    );

                    contract_state.sum_of_burns =
                        contract_state.sum_of_burns + &BigInt::from(input.amount);
                }
            }
            Command::Burn(input) => {
                let r = token_client.try_burn(&accounts[input.from_account_index], &input.amount);

                if input.amount < 0 {
                    assert!(r.is_err());
                }

                verify_token_contract_result(&env, &r);

                log_result("burn", &r);

                if let Ok(r) = r {
                    let r = r.unwrap();

                    contract_state.sub_balance(&accounts[input.from_account_index], input.amount);

                    contract_state.sum_of_burns =
                        contract_state.sum_of_burns + &BigInt::from(input.amount);
                }
            }
            Command::AdvanceLedgers(cmd_input) => {
                let to_ledger = env
                    .ledger()
                    .sequence()
                    .checked_add(cmd_input.ledgers)
                    .expect("end of time");

                env = advance_time_to(&config, env, &token_contract_id_bytes, to_ledger);
                // NB: This env is reconstructed and all previous env-based objects are invalid

                current_state =
                    CurrentState::new(&config, &env, &input.addresses, &token_contract_id_bytes);

                // update saved allowance number after advance ledgers
                // fixme track expiration ledger instead of asking the contract
                {
                    let pairs = current_state
                        .accounts
                        .iter()
                        .cartesian_product(current_state.accounts.iter());
                    for (addr1, addr2) in pairs {
                        contract_state.set_allowance(
                            addr1,
                            addr2,
                            current_state.token_client.allowance(addr1, addr2),
                        );
                    }
                }
            }
        }

        assert_state(&contract_state, &current_state);
    }

    // eprintln!("results: {results:?}");

    Corpus::Keep
}

pub struct ContractState {
    name: RustVec<u8>,
    symbol: RustVec<u8>,
    decimals: u32,
    balances: BTreeMap<RustVec<u8>, i128>,
    allowances: BTreeMap<(RustVec<u8>, RustVec<u8>), i128>, // (from, spender)
    sum_of_mints: BigInt,
    sum_of_burns: BigInt,
}

impl ContractState {
    fn init(env: &Env) -> Self {
        ContractState {
            name: Vec::<u8>::new(),
            symbol: Vec::<u8>::new(),
            decimals: 0,
            balances: BTreeMap::default(),
            allowances: BTreeMap::default(),
            sum_of_mints: BigInt::default(),
            sum_of_burns: BigInt::default(),
        }
    }

    fn get_balance(&self, addr: &Address) -> i128 {
        let addr_bytes = address_to_bytes(addr);
        self.balances.get(&addr_bytes).copied().unwrap_or(0)
    }

    fn sub_balance(&mut self, addr: &Address, amount: i128) {
        let addr_bytes = address_to_bytes(addr);
        let balance = self.get_balance(addr);
        let new_balance = balance.checked_sub(amount).expect("overflow");
        assert!(new_balance >= 0);
        self.balances.insert(addr_bytes, new_balance);
    }

    fn add_balance(&mut self, addr: &Address, amount: i128) {
        let addr_bytes = address_to_bytes(addr);
        let balance = self.get_balance(addr);
        let new_balance = balance.checked_add(amount).expect("overflow");
        assert!(new_balance >= 0);
        self.balances.insert(addr_bytes, new_balance);
    }

    fn set_allowance(&mut self, from: &Address, spender: &Address, amount: i128) {
        assert!(amount >= 0);
        let from_bytes = address_to_bytes(from);
        let spender_bytes = address_to_bytes(spender);
        self.allowances.insert((from_bytes, spender_bytes), amount);
    }

    fn get_allowance(&self, from: &Address, spender: &Address) -> i128 {
        let from_bytes = address_to_bytes(from);
        let spender_bytes = address_to_bytes(spender);
        self.allowances
            .get(&(from_bytes, spender_bytes))
            .copied()
            .unwrap_or(0)
    }

    fn sub_allowance(&mut self, from: &Address, spender: &Address, amount: i128) {
        let allowance = self.get_allowance(from, spender);
        let new_allowance = allowance.checked_sub(amount).expect("overflow");
        assert!(new_allowance >= 0);
        self.set_allowance(from, spender, new_allowance);
    }
}

struct CurrentState<'a> {
    admin: Address,
    accounts: Vec<Address>,
    admin_client: Box<dyn TokenAdminClient<'a> + 'a>,
    token_client: Client<'a>,
}

impl<'a> CurrentState<'a> {
    fn new(
        config: &Config,
        env: &Env,
        accounts: &[<Address as SorobanArbitrary>::Prototype],
        token_contract_id_bytes: &[u8],
    ) -> Self {
        let admin = Address::from_val(env, &accounts[0]);
        let accounts = accounts
            .iter()
            .map(|a| Address::from_val(env, a))
            .collect::<Vec<_>>();

        let token_contract_id =
            Address::from_string_bytes(&Bytes::from_slice(env, &token_contract_id_bytes));
        let admin_client = config.new_admin_client(env, &token_contract_id);
        let token_client = Client::new(env, &token_contract_id);

        CurrentState {
            admin,
            accounts,
            admin_client,
            token_client,
        }
    }
}

fn assert_state(contract: &ContractState, current: &CurrentState) {
    let token_client = &current.token_client;

    assert!(contract.name.eq(&string_to_bytes(token_client.name())));
    assert!(contract.symbol.eq(&string_to_bytes(token_client.symbol())));
    assert_eq!(contract.decimals, token_client.decimals());

    for addr in &current.accounts {
        assert_eq!(contract.get_balance(addr), token_client.balance(addr));
        assert!(token_client.balance(addr) >= 0)
    }

    let pairs = current
        .accounts
        .iter()
        .cartesian_product(current.accounts.iter());

    for (addr1, addr2) in pairs {
        assert_eq!(
            contract.get_allowance(addr1, addr2),
            token_client.allowance(addr1, addr2),
        );
    }

    let sum_of_balances_0 = &contract.sum_of_mints - &contract.sum_of_burns;
    let sum_of_balances_1 = current
        .accounts
        .iter()
        .map(|a| BigInt::from(token_client.balance(&a)))
        .sum();

    assert_eq!(sum_of_balances_0, sum_of_balances_1);
}

fn string_to_bytes(s: String) -> RustVec<u8> {
    let mut out = vec![0; s.len() as usize];
    s.copy_into_slice(&mut out);

    out
}

fn require_unique_addresses(addrs: &[Address]) -> bool {
    for addr1 in addrs {
        let count = addrs.iter().filter(|a| a == &addr1).count();
        if count > 1 {
            return false;
        }
    }
    true
}

fn require_contract_addresses(addrs: &[Address]) -> bool {
    use stellar_strkey::*;
    for addr in addrs {
        let addr_string = addr.to_string();
        let mut addr_buf = vec![0; addr_string.len() as usize];
        addr_string.copy_into_slice(&mut addr_buf);
        let addr_string = std::str::from_utf8(&addr_buf).unwrap();
        let strkey = Strkey::from_string(&addr_string).unwrap();
        match strkey {
            Strkey::Contract(_) => {}
            _ => {
                return false;
            }
        }
    }
    true
}

fn create_env() -> Env {
    Env::default()
}

fn advance_env(prev_env: Env, ledgers: u32) -> Env {
    use soroban_sdk::testutils::Ledger as _;

    let secs_per_ledger = {
        let secs_per_day = 60 * 60 * 24;
        let ledgers_per_day = DAY_IN_LEDGERS as u64;
        secs_per_day / ledgers_per_day
    };
    let ledger_time = secs_per_ledger
        .checked_mul(ledgers as u64)
        .expect("end of time");

    let use_snapshot = true;

    if !use_snapshot {
        let mut env = prev_env.clone();
        env.ledger().with_mut(|ledger| {
            ledger.sequence_number = ledger
                .sequence_number
                .checked_add(ledgers)
                .expect("end of time");
            ledger.timestamp = ledger
                .timestamp
                .checked_add(ledger_time)
                .expect("end of time");
        });

        env
    } else {
        let mut snapshot = prev_env.to_snapshot();
        snapshot.ledger.sequence_number = snapshot
            .ledger
            .sequence_number
            .checked_add(ledgers)
            .expect("end of time");
        snapshot.ledger.timestamp = snapshot
            .ledger
            .timestamp
            .checked_add(ledger_time)
            .expect("end of time");

        let env = Env::from_snapshot(snapshot);

        env
    }
}

/// Advance time, but do it in increments, periodically pinging the contract to
/// keep it alive.
fn advance_time_to(
    config: &Config,
    mut env: Env,
    token_contract_id_bytes: &[u8],
    to_ledger: u32,
) -> Env {
    loop {
        let curr_ledger = env.ledger().get().sequence_number;
        assert!(curr_ledger < to_ledger);

        let next_ledger = curr_ledger
            .checked_add(DAY_IN_LEDGERS)
            .expect("end of time");
        let next_ledger = next_ledger.min(to_ledger);

        let advance_ledgers = next_ledger - curr_ledger;

        env = advance_env(env, advance_ledgers);

        let token_contract_id =
            Address::from_string_bytes(&Bytes::from_slice(&env, &token_contract_id_bytes));
        config.reregister_contract(&env, &token_contract_id);

        if next_ledger == to_ledger {
            break;
        } else {
            // Keep the contract alive
            let token_contract_id =
                Address::from_string_bytes(&Bytes::from_slice(&env, &token_contract_id_bytes));
            let token_client = Client::new(&env, &token_contract_id);
            let r = token_client.try_allowance(&Address::generate(&env), &Address::generate(&env));
            assert!(r.is_ok());
        }
    }

    env
}

fn address_to_bytes(addr: &Address) -> RustVec<u8> {
    let addr_str = addr.to_string();
    let mut buf = vec![0; addr_str.len() as usize];
    addr_str.copy_into_slice(&mut buf);
    buf
}

fn verify_token_contract_result(env: &Env, r: &TokenContractResult) {
    use soroban_sdk::testutils::Events;
    use soroban_sdk::xdr::{ScErrorCode, ScErrorType};
    match r {
        Err(Ok(e)) => {
            if e.is_type(ScErrorType::WasmVm) && e.is_code(ScErrorCode::InvalidAction) {
                let msg = "contract failed with InvalidAction - unexpected panic?";
                eprintln!("{msg}");
                eprintln!("recent events (10):");
                for (i, event) in env.events().all().iter().rev().take(10).enumerate() {
                    eprintln!("{i}: {event:?}");
                }
                panic!("{msg}");
            }
        }
        _ => {}
    }
}

/*

possible assertions

// - allowances can't be greater than balance?
// - no negative balances?
// - make assertions about name/decimals/symbol
// - assertions about negative amounts
- predict if a call will succeed based on ContractState

todo

- use auths correctly
- allow other address types

*/