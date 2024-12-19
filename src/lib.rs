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
use near_sdk::PromiseResult;


const DAY: u64 = 86400; // Seconds in a day
const MONTH: u64 = 30 * DAY; // Seconds in a month
const MONTHLY_REWARD: u128 = 1_666_666_666_67; // Monthly reward pool

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

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct NFTStakingContract {
    pub owner: AccountId,
    sin_token: AccountId,
    sin_nft_contract: AccountId,
    pub stakers: UnorderedMap<AccountId, StakerInfo>,
    pub reward_pool: u128,
    pub last_distributed: u64,
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
            nft_weights,
        }
    }

    #[payable]
    pub fn stake_nfts(&mut self, nft_ids: Vec<String>) {
        let staker_id = env::predecessor_account_id();
        let start_timestamp = env::block_timestamp();
    
        // Ensure the user has provided at least one NFT ID
        assert!(!nft_ids.is_empty(), "NFT IDs cannot be empty");
    
        // Batch approve all NFTs
        Promise::new(self.sin_nft_contract.clone()).function_call(
            "nft_batch_transfer".to_string(),
            serde_json::to_vec(&json!({
                "token_ids": nft_ids
                    .iter()
                    .map(|nft_id| (nft_id.clone(), env::current_account_id()))
                    .collect::<Vec<_>>(),
            }))
            .expect("Failed to serialize nft_batch_transfer arguments"),
            NearToken::from_yoctonear(1), // Attach 1 yoctoNEAR
            Gas::from_tgas(100), // Adjust gas as needed for batch transfer
        );
    
        // Default to "Drone" type for now; will be updated during metadata fetching
        let nft_types: HashMap<String, String> = nft_ids
            .iter()
            .map(|nft_id| (nft_id.clone(), "Drone".to_string()))
            .collect();
    
        // Update staker info
        let mut staker_info = self.stakers.get(&staker_id).unwrap_or_else(|| StakerInfo {
            stakes: Vector::new(format!("stakes_{}", staker_id).as_bytes().to_vec()),
            total_rewards_claimed: 0,
        });
    
        staker_info.stakes.push(&NFTStakingRecord {
            nft_ids,
            nft_types,
            start_timestamp,
            lockup_period: MONTH,
            claimed_rewards: 0,
        });
    
        self.stakers.insert(&staker_id, &staker_info);
    
        env::log_str(&format!("NFTs staked by {} successfully", staker_id));
    }


    #[private]
    pub fn on_fetch_metadata(
        &mut self,
        staker_id: AccountId,
        nft_id: String,
        start_timestamp: u64,
    ) {
        // Verify promise result
        assert_eq!(
            env::promise_results_count(),
            1,
            "Expected exactly one promise result"
        );

        let nft_metadata: Value = match env::promise_result(0) {
            PromiseResult::Successful(result) => {
                let metadata = serde_json::from_slice::<Value>(&result)
                    .expect("Failed to parse NFT metadata");
        
                // Print the metadata to the logs
                env::log_str(&format!(
                    "Fetched NFT Metadata: {}",
                    serde_json::to_string_pretty(&metadata).unwrap()
                ));
        
                metadata
            }
            _ => env::panic_str("Failed to fetch NFT metadata"),
        };
        // Validate ownership
        assert_eq!(
            nft_metadata["owner_id"].as_str().unwrap(),
            staker_id,
            "You do not own NFT ID: {}",
            nft_id
        );

    // Classify NFT type
    let nft_type = Self::classify_nft_type(&nft_metadata);

    // Fetch staker info or create new record
    let mut staker_info = self.stakers.get(&staker_id).unwrap_or_else(|| StakerInfo {
        stakes: Vector::new(format!("stakes_{}", staker_id).as_bytes().to_vec()),
        total_rewards_claimed: 0,
    });

    // Initialize nft_types with explicit type
    let mut nft_types: HashMap<String, String> = HashMap::new();
    nft_types.insert(nft_id.clone(), nft_type);

    // Add the NFT staking record
    staker_info.stakes.push(&NFTStakingRecord {
        nft_ids: vec![nft_id],
        nft_types,
        start_timestamp,
        lockup_period: MONTH,
        claimed_rewards: 0,
    });

    self.stakers.insert(&staker_id, &staker_info);
}
    pub fn classify_nft_type(meta: &Value) -> String {
        // Safely access reference_blob and attributes
        let attributes = match meta.get("reference_blob")
            .and_then(|blob| blob.get("attributes"))
            .and_then(|attrs| attrs.as_array()) 
        {
            Some(attrs) => attrs,
            None => return "Drone".to_string(), // Default to Drone if attributes are missing
        };

        // Search for specific attributes to classify the NFT
        for attribute in attributes {
            if let (Some(trait_type), Some(value)) = (
                attribute.get("trait_type").and_then(|t| t.as_str()),
                attribute.get("value").and_then(|v| v.as_str()),
            ) {
                if trait_type == "Body" && value == "Queen" {
                    return "Queen".to_string();
                } else if trait_type == "Wings" && value == "Diamond" {
                    return "Worker".to_string();
                }
            }
        }

        // Default to Drone if no specific match
        "Drone".to_string()
    }

    pub fn distribute_rewards(&mut self) {
        assert_eq!(
            env::predecessor_account_id(),
            self.owner,
            "Only owner can distribute rewards"
        );

        let reward_pool = MONTHLY_REWARD;
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

        for nft_id in nft_ids {
            Promise::new(self.sin_nft_contract.clone()).function_call(
                "nft_transfer".to_string(),
                serde_json::to_vec(&json!({
                    "receiver_id": staker_id,
                    "token_id": nft_id,
                }))
                .unwrap(),
                NearToken::from_yoctonear(1),
                Gas::from_tgas(30),
            );
        }
    }

    pub fn get_staking_info(&self, staker_id: AccountId) -> Vec<serde_json::Value> {
        let staker_info = self.stakers.get(&staker_id).expect("Staker not found");

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
}