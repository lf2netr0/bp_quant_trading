# HFT Grid Trading Bot for Backpack Exchange

## Installation

1. Install Rust and Cargo: https://www.rust-lang.org/tools/install
2. Clone this repository
3. Update `config.toml` with your API credentials
4. Build the bot: `cargo build --release`

## Usage

1. Run the bot: `cargo run --release`
2. Monitor the logs for trading activity
3. Stop the bot by pressing Ctrl+C

## Features

- High-frequency grid trading strategy
- WebSocket connectivity for real-time updates
- Robust error handling and reconnection logic
- Risk management with panic mode
- Configurable grid parameters

## Configuration

Edit `config.toml` to customize the bot:
- API credentials
- Trading symbol (default: SOL_USDC)
- Grid boundaries (+10% / -10% by default)
- Number of grid levels (2000 by default)
- Price and quantity precision
- Minimum trade quantity\



