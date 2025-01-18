use near_sdk::{
    borsh::{self, BorshDeserialize, BorshSerialize},
    collections::{UnorderedMap, Vector},
    env, near_bindgen, AccountId, PanicOnDefault, Promise, NearToken
};
use near_sdk::{json_types::U128, Gas};
use serde_json::Value;
use std::collections::HashMap;
use serde_json::json;
use near_sdk::serde::{Deserialize, Serialize};
use near_contract_standards::fungible_token::Balance;


const DAY: u64 = 86400; // Seconds in a day
const MONTH: u64 = 30 * DAY; // Seconds in a month

#[derive(BorshDeserialize, BorshSerialize, Clone, Serialize, Deserialize)]
pub struct NFTStakingRecord {
    pub nft_ids: Vec<String>, // List of NFT IDs in the staking
    pub nft_types: HashMap<String, String>, // Map of NFT ID -> Type (Queen, Worker, Drone)
    pub start_timestamp: u64,
    pub lockup_period: u64,
    pub claimed_rewards: u128,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub struct StakerInfo {
    pub stakes: Vector<NFTStakingRecord>,
    pub total_rewards_claimed: u128,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct FundingRecord {
    pub amount: Balance,
    pub timestamp: u64,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub struct RewardDistribution {
    pub total_reward_pool: Balance,
    pub last_distributed: u64, // Timestamp of last reward distribution
    pub funding_records: Vector<FundingRecord>, // Track funding history
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct NFTStakingContract {
    pub owner: AccountId,
    sin_token: AccountId,
    sin_nft_contract: AccountId,
    pub stakers: UnorderedMap<AccountId, StakerInfo>,
    pub reward_pool: u128,
    pub last_distributed: u64,
    pub reward_distribution: RewardDistribution,
    pub nft_weights: HashMap<String, u32>, // Map for NFT type -> Weight
}

#[near_bindgen]
impl NFTStakingContract {
    #[init]
    pub fn new(owner: AccountId, sin_token: AccountId, sin_nft_contract: AccountId) -> Self {
        let mut nft_weights = HashMap::new();
        nft_weights.insert("Queen".to_string(), 50);
        nft_weights.insert("Worker".to_string(), 30);
        nft_weights.insert("Drone".to_string(), 20);

        Self {
            owner,
            sin_token,
            sin_nft_contract,
            stakers: UnorderedMap::new(b"s".to_vec()),
            reward_pool: 0,
            last_distributed: env::block_timestamp(),
            reward_distribution: RewardDistribution {
                total_reward_pool: 0,
                last_distributed: env::block_timestamp(),
                funding_records: Vector::new(b"fundings".to_vec()),
            },
            nft_weights,
        }
    }

    #[payable]
    pub fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> U128 {
        env::log_str(&format!("Received {} tokens from {}", amount.0, sender_id));
        assert_eq!(
            sender_id,
            self.owner,
            "Only Only contract owners are allowed to fund this reward pool"
        );
        assert_eq!(
            env::predecessor_account_id(),
            self.sin_token,
            "Only SIN tokens are accepted for funding"
        );
        assert!(amount.0 > 0, "Funding amount must be greater than zero");

        // Update total reward pool
        self.reward_distribution.total_reward_pool += amount.0;

        // Track funding record
        self.reward_distribution.funding_records.push(&FundingRecord {
            amount: amount.0,
            timestamp: env::block_timestamp(),
        });

        env::log_str(&format!(
            "Reward pool funded with {} SIN tokens by {} with message {}",
            amount.0, env::predecessor_account_id(), msg
        ));
        // Return 0 to indicate all tokens were accepted
        U128(0)
    }

    #[payable]
    pub fn nft_on_transfer(&mut self, sender_id: AccountId, token_id: String, msg: String) -> bool {
        env::log_str(&format!("Received NFT {} from {} with metadata {}", token_id, sender_id, msg));
        
        // Ensure the call is from the authorized NFT contract
        assert_eq!(
            env::predecessor_account_id(),
            self.sin_nft_contract,
            "NFT can only be transferred from the SIN NFT contract"
        );
    
        // Parse the metadata directly from the msg parameter
        let metadata: Value = serde_json::from_str(&msg).expect("Failed to parse metadata from msg");
    
        // Classify the NFT type
        let nft_type = Self::classify_nft_type(&metadata);
    
        // Update staker information
        let mut staker_info = self.stakers.get(&sender_id).unwrap_or_else(|| StakerInfo {
            stakes: Vector::new(format!("stakes_{}", sender_id).as_bytes().to_vec()),
            total_rewards_claimed: 0,
        });
    
        let mut nft_types = HashMap::new();
        nft_types.insert(token_id.clone(), nft_type);
    
        staker_info.stakes.push(&NFTStakingRecord {
            nft_ids: vec![token_id.clone()],
            nft_types,
            start_timestamp: env::block_timestamp(),
            lockup_period: MONTH,
            claimed_rewards: 0,
        });
    
        self.stakers.insert(&sender_id, &staker_info);
    
        env::log_str(&format!("NFT {} successfully staked by {}", token_id, sender_id));
    
        // Returning `false` ensures the NFT is not refunded
        false
    }


    pub fn classify_nft_type(meta: &Value) -> String {
        // Safely access reference_blob and attributes
        let binding = vec![];
        let attributes = meta
            .get("reference_blob")
            .and_then(|blob| blob.get("attributes"))
            .and_then(|attrs| attrs.as_array())
            .unwrap_or(&binding);
    
        let mut is_queen = false;
        let mut is_worker = false;
    
        for attribute in attributes {
            if let (Some(trait_type), Some(value)) = (
                attribute.get("trait_type").and_then(|t| t.as_str()),
                attribute.get("value").and_then(|v| v.as_str()),
            ) {
                if trait_type == "Body" && value == "Queen" {
                    is_queen = true;
                } else if trait_type == "Wings" && value == "Diamond" {
                    is_worker = true;
                }
            }
        }
    
        // Prioritize Queen over Worker
        if is_queen {
            "Queen".to_string()
        } else if is_worker {
            "Worker".to_string()
        } else {
            "Drone".to_string() // Default to Drone if no specific match
        }
    }

    pub fn distribute_rewards(&mut self, amount: U128) {
        assert_eq!(
            env::predecessor_account_id(),
            self.owner,
            "Only owner can distribute rewards"
        );

        assert!(
            amount.0 <= self.reward_distribution.total_reward_pool,
            "Insufficient funds in the reward pool for distribution"
        );

        let reward_pool = amount.0;
        let mut total_tpes = 0.0;
        let mut staker_tpes: HashMap<AccountId, Vec<(usize, f64)>> = HashMap::new();

        for (staker_id, staker_info) in self.stakers.iter() {
            let mut stakes_tpes = vec![];
        
            for i in 0..staker_info.stakes.len() {
                let stake = staker_info.stakes.get(i as u64).unwrap();
                let mut tpes = 0.0;
        
                for (_nft_id, nft_type) in stake.nft_types.iter() {
                    let weight = self.nft_weights.get(nft_type).unwrap_or(&0);
                    tpes += *weight as f64;
                }
        
                // Cast `i` to `usize` for compatibility
                stakes_tpes.push((i as usize, tpes));
                total_tpes += tpes;
            }
        
            staker_tpes.insert(staker_id.clone(), stakes_tpes);
        }

        for (staker_id, stakes_tpes) in staker_tpes {
            let mut staker_info = self.stakers.get(&staker_id).unwrap();

            for (i, tpes) in stakes_tpes {
                let reward_percentage = reward_pool as f64 / total_tpes;
                let reward = (tpes * reward_percentage) as u128;

                let mut stake = staker_info.stakes.get(i as u64).unwrap();
                stake.claimed_rewards += reward;
                staker_info.stakes.replace(i as u64, &stake);
            }
            self.stakers.insert(&staker_id, &staker_info);
        }
        self.reward_distribution.total_reward_pool -= reward_pool;
        self.last_distributed = env::block_timestamp();
    }

    pub fn claim_reward(&mut self, stake_index: u64) {
        let staker_id = env::predecessor_account_id();
        let mut staker_info = self.stakers.get(&staker_id).expect("Staker not found");

        assert!(
            stake_index < staker_info.stakes.len(),
            "Invalid staking record index"
        );

        let mut stake = staker_info.stakes.get(stake_index).unwrap();
        let rewards_to_claim = stake.claimed_rewards;

        assert!(rewards_to_claim > 0, "No rewards available to claim");

        stake.claimed_rewards = 0;
        staker_info.total_rewards_claimed += rewards_to_claim;
        staker_info.stakes.replace(stake_index, &stake);
        self.stakers.insert(&staker_id, &staker_info);

        Promise::new(self.sin_token.clone()).function_call(
            "ft_transfer".to_string(),
            serde_json::to_vec(&json!({
                "receiver_id": staker_id,
                "amount": U128(rewards_to_claim),
            }))
            .unwrap(),
            NearToken::from_yoctonear(1), // Attach 1 yoctoNEAR
            Gas::from_tgas(50),
        );
    }

    pub fn unstake_nfts(&mut self, stake_index: u64) {
        let staker_id = env::predecessor_account_id();
        let mut staker_info = self.stakers.get(&staker_id).expect("Staker not found");

        assert!(
            stake_index < staker_info.stakes.len(),
            "Invalid staking record index"
        );

        let stake = staker_info.stakes.get(stake_index).unwrap();
        let current_time = env::block_timestamp();
        assert!(
            current_time >= stake.start_timestamp + stake.lockup_period,
            "Cannot unstake before lockup period"
        );

        let nft_ids = stake.nft_ids.clone();
        staker_info.stakes.swap_remove(stake_index);
        self.stakers.insert(&staker_id, &staker_info);

        let transfer_data: Vec<(String, AccountId)> = nft_ids
            .iter()
            .map(|nft_id| (nft_id.clone(), staker_id.clone()))
            .collect();

        Promise::new(self.sin_nft_contract.clone()).function_call(
            "nft_batch_transfer".to_string(),
            serde_json::to_vec(&json!({ "token_ids": transfer_data })).unwrap(),
            NearToken::from_yoctonear(1),
            Gas::from_tgas(100),
        );
    }

    pub fn get_staking_info(&self, staker_id: AccountId) -> Vec<serde_json::Value> {
        if let Some(staker_info) = self.stakers.get(&staker_id) {
            staker_info
                .stakes
                .iter()
                .map(|stake| {
                    // Aggregate NFT type counts
                    let mut queen_count = 0;
                    let mut worker_count = 0;
                    let mut drone_count = 0;
    
                    for (_, nft_type) in stake.nft_types.iter() {
                        match nft_type.as_str() {
                            "Queen" => queen_count += 1,
                            "Worker" => worker_count += 1,
                            "Drone" => drone_count += 1,
                            _ => (),
                        }
                    }
    
                    // Return the summarized data
                    json!({
                        "nft_ids": stake.nft_ids,
                        "queen": queen_count,
                        "worker": worker_count,
                        "drone": drone_count,
                        "start_timestamp": stake.start_timestamp,
                        "lockup_period": stake.lockup_period,
                        "claimed_rewards": stake.claimed_rewards
                    })
                })
                .collect()
        } else {
            vec![] // Return empty if no staking info is found
        }
    }

    pub fn get_last_reward_distribution(&self) -> u64 {
        self.last_distributed
    }

    pub fn get_next_reward_distribution(&self) -> u64 {
        let now = env::block_timestamp();
        let next_distribution = self.last_distributed + MONTH;
        if next_distribution > now {
            (next_distribution - now) / DAY
        } else {
            0
        }
    }
    pub fn get_available_reward(&self) -> u128 {
        self.reward_distribution.total_reward_pool
    }
    pub fn get_funding_details(&self) -> Vec<FundingRecord> {
        self.reward_distribution
            .funding_records
            .iter()
            .collect::<Vec<FundingRecord>>()
    }
}