use near_sdk::{env, near_bindgen, AccountId, Promise, PanicOnDefault, NearToken, Gas};
use near_sdk::collections::UnorderedMap;
use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use serde::{Serialize, Deserialize};
use serde_json::json;

type Balance = u128;
const ONE_MONTH_IN_SECONDS: u64 = 2_592_000;
const REWARD_POOL_PER_MONTH: Balance = 1_666_666_666_670_000_000_000_000;
const NFT_CONTRACT_ADDRESS: &str = "sin-nft.bodega-lab.near";
const SIN_TOKEN_CONTRACT: &str = "sin.token.contract";
const GAS_FOR_NFT_FETCH: Gas = Gas::from_tgas(10);

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct NftStakingContract {
    owner: AccountId,
    funding_wallet: AccountId,
    staking_info: UnorderedMap<AccountId, Vec<NftStake>>,
    total_points: u128,
    reward_pool: Balance,
    last_reward_timestamp: u64,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct NftStake {
    nft_id: String,
    nft_type: NftType,
    weight: u128,
    staked_at: u64,
    claimed: bool,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(crate = "near_sdk::serde")]
pub enum NftType {
    Queen,
    Worker,
    Drone,
}

#[near_bindgen]
impl NftStakingContract {
    #[init]
    pub fn new(owner: AccountId, funding_wallet: AccountId) -> Self {
        Self {
            owner,
            funding_wallet,
            staking_info: UnorderedMap::new(b"s"),
            total_points: 0,
            reward_pool: REWARD_POOL_PER_MONTH,
            last_reward_timestamp: env::block_timestamp() / 1_000_000_000,
        }
    }

    #[payable]
    pub fn fund_reward_pool(&mut self) {
        let account_id = env::predecessor_account_id();
        assert_eq!(account_id, self.owner, "Only the contract owner can fund the reward pool.");
        assert_eq!(
            env::predecessor_account_id(),
            SIN_TOKEN_CONTRACT,
            "Funds can only come from the SIN token contract."
        );
        let attached_amount = env::attached_deposit();
        assert!(attached_amount > NearToken::from_yoctonear(0), "You need to attach some tokens to fund the reward pool.");
        self.reward_pool +=  NearToken::as_yoctonear(&attached_amount);
        env::log_str(&format!("{} funded the reward pool with {} SIN tokens", account_id, attached_amount));
    }

    pub fn stake_nft(&mut self, nft_id: String) -> Promise {
        let account_id = env::predecessor_account_id();
    
        Promise::new(NFT_CONTRACT_ADDRESS.parse().unwrap()).function_call(
            "nft_token".to_string(),
            json!({ "token_id": nft_id }).to_string().into_bytes(),
            NearToken::from_yoctonear(0),
            env::prepaid_gas().saturating_sub(GAS_FOR_NFT_FETCH),
        )
        .then(Self::ext(env::current_account_id()).stake_nft_callback(account_id, nft_id))
    }
    
    #[private]
    pub fn stake_nft_callback(&mut self, account_id: AccountId, nft_id: String) {
        assert_eq!(env::promise_results_count(), 1, "Expected one promise result");
    
        match env::promise_result(0) {
            near_sdk::PromiseResult::Successful(result) => {
                let nft_metadata: serde_json::Value =
                    serde_json::from_slice(&result).expect("Failed to parse metadata");
                let nft_type = self.get_nft_type(&nft_metadata);
    
                let weight = match nft_type {
                    NftType::Queen => 50,
                    NftType::Worker => 30,
                    NftType::Drone => 20,
                };
    
                let mut user_stakes = self.staking_info.get(&account_id).unwrap_or_default();
                user_stakes.push(NftStake {
                    nft_id: nft_id.clone(),
                    nft_type: nft_type.clone(),
                    weight,
                    staked_at: env::block_timestamp() / 1_000_000_000,
                    claimed: false,
                });
                self.staking_info.insert(&account_id, &user_stakes);
                self.total_points += weight;
    
                env::log_str(&format!(
                    "Successfully staked NFT {} of type {:?} with weight {}",
                    nft_id, nft_type, weight
                ));
            }
            _ => env::panic_str("Failed to fetch NFT metadata"),
        }
    }

    pub fn claim_rewards(&mut self) {
        let account_id = env::predecessor_account_id();
        let mut user_stakes = self.staking_info.get(&account_id).expect("No stakes found.");
        let mut total_rewards = 0;

        for stake in user_stakes.iter_mut() {
            if !stake.claimed && self.is_lockup_complete(stake.staked_at) {
                let reward_percentage = self.reward_pool as f64 / self.total_points as f64;
                let reward = (stake.weight as f64 * reward_percentage) as u128;
                total_rewards += reward;
                stake.claimed = true;
            }
        }

        assert!(total_rewards > 0, "No rewards available to claim.");
        self.staking_info.insert(&account_id, &user_stakes);
        self.transfer_rewards(account_id.clone(), total_rewards);
        env::log_str(&format!("{} claimed {} SIN tokens as rewards", account_id, total_rewards));
    }

    pub fn unstake(&mut self, nft_id: String) {
        let account_id = env::predecessor_account_id();
        let mut user_stakes = self.staking_info.get(&account_id).expect("No stakes found.");
        let index = user_stakes.iter().position(|stake| stake.nft_id == nft_id).expect("NFT not staked.");

        let stake = user_stakes.remove(index);
        assert!(self.is_lockup_complete(stake.staked_at), "Lock-up period not complete.");

        self.total_points -= stake.weight;
        self.staking_info.insert(&account_id, &user_stakes);

        env::log_str(&format!("Successfully unstaked NFT {}", nft_id));
    }

    #[private]
    pub fn nft_owner_callback(&mut self, nft_id: String) -> AccountId {
        assert_eq!(
            env::promise_results_count(),
            1,
            "Expected one promise result"
        );

        match env::promise_result(0) {
            near_sdk::PromiseResult::Successful(result) => {
                let owner_data: serde_json::Value =
                    serde_json::from_slice(&result).expect("Failed to parse owner data");
                let owner = owner_data
                    .get("owner_id")
                    .expect("No owner_id field found")
                    .as_str()
                    .expect("Invalid owner ID format")
                    .to_string();

                AccountId::new_unvalidated(owner)
            }
            _ => env::panic_str("Failed to fetch NFT owner"),
        }
    }

    pub fn get_nft_owner(&self, nft_id: String) -> Promise {
        Promise::new(NFT_CONTRACT_ADDRESS.parse().unwrap()).function_call(
            "nft_token".to_string(),
            json!({ "token_id": nft_id }).to_string().into_bytes(),
            NearToken::from_yoctonear(0),
            env::prepaid_gas().saturating_sub(GAS_FOR_NFT_FETCH),
        )
        .then(Self::ext(env::current_account_id()).nft_owner_callback(nft_id))
    }

    #[private]
    pub fn nft_metadata_callback(&mut self, nft_id: String) -> serde_json::Value {
        assert_eq!(
            env::promise_results_count(),
            1,
            "Expected one promise result"
        );

        match env::promise_result(0) {
            near_sdk::PromiseResult::Successful(result) => {
                let metadata: serde_json::Value =
                    serde_json::from_slice(&result).expect("Failed to parse metadata");
                metadata
            }
            _ => env::panic_str("Failed to fetch NFT metadata"),
        }
    }

    pub fn get_nft_metadata(&self, nft_id: String) -> Promise {
        Promise::new(NFT_CONTRACT_ADDRESS.parse().unwrap()).function_call(
            "nft_token".to_string(),
            json!({ "token_id": nft_id }).to_string().into_bytes(),
            NearToken::from_yoctonear(0),
            env::prepaid_gas().saturating_sub(GAS_FOR_NFT_FETCH), // Use the div method for Gas
        )
        .then(Self::ext(env::current_account_id()).nft_metadata_callback(nft_id))
    }

    fn get_nft_type(&self, metadata: &serde_json::Value) -> NftType {
        let body_value = metadata["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|attr| attr["trait_type"] == "Body")
            .map(|attr| attr["value"].as_str().unwrap())
            .unwrap_or("");
        let wings_value = metadata["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|attr| attr["trait_type"] == "Wings")
            .map(|attr| attr["value"].as_str().unwrap())
            .unwrap_or("");

        if body_value == "Queen" {
            NftType::Queen
        } else if wings_value == "Diamond" {
            NftType::Worker
        } else {
            NftType::Drone
        }
    }

    fn is_lockup_complete(&self, staked_at: u64) -> bool {
        let current_time = env::block_timestamp() / 1_000_000_000;
        current_time >= staked_at + ONE_MONTH_IN_SECONDS
    }

    fn transfer_rewards(&self, to: AccountId, amount: Balance) {
        Promise::new(to).transfer(NearToken::from_yoctonear(amount));
    }

    pub fn get_total_staked_points(&self) -> u128 {
        self.total_points
    }

    pub fn get_reward_pool_balance(&self) -> Balance {
        self.reward_pool
    }
    
    pub fn get_user_stakes(&self, account_id: AccountId) -> Vec<NftStake> {
        self.staking_info.get(&account_id).unwrap_or_default()
    }

}