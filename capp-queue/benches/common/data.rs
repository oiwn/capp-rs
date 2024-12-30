use fake::faker::internet::en::Username;
use fake::{Dummy, Faker};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Dummy)]
pub struct BenchTaskData {
    #[dummy(faker = "1000..2000")]
    pub timestamp: u64,
    #[dummy(faker = "0.0..1.0")]
    pub priority: f64,
    #[dummy(faker = "Username()")]
    pub category: String,
    #[dummy(faker = "0..10")]
    pub attempts: u64,
    #[dummy(faker = "Username()")] // Using Username as example URL for simplicity
    pub url: String,
}

pub fn generate_test_data(count: usize) -> Vec<BenchTaskData> {
    (0..count).map(|_| BenchTaskData::dummy(&Faker)).collect()
}
