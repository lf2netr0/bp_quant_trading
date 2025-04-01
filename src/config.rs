// config.rs - Configuration for the bot

use serde::Deserialize;
use std::error::Error;
use std::fs;

#[derive(Deserialize, Clone)]
pub struct Config {
    // API credentials
    pub api_key: String,
    pub api_secret: String,

    // Exchange settings
    pub base_url: String,
    pub ws_url: String,

    // Trading parameters
    pub symbol: String,
    pub grid_upper_bound: f64, // +10%
    pub grid_lower_bound: f64, // -10%
    pub grid_levels: usize,    // 2000
    pub active_orders: usize,  // 6

    // Order parameters
    pub price_precision: usize,    // 2 decimal places
    pub quantity_precision: usize, // 2 decimal places
    pub min_quantity: f64,         // 0.01
}

impl Config {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}
