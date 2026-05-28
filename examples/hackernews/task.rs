use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HnTask {
    Listing { page: u32 },
    Item { id: u64 },
}
