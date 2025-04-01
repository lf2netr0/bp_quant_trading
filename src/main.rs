// main.rs - Entry point with risk management

use std::error::Error;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tokio::time::Duration;
use std::io::Write;
use log::{info, error, warn, debug};
use env_logger::Builder;
use chrono::Local;

// Import our modules
mod api;
mod config;
mod order_manager;
mod risk_manager;
mod strategy;

use api::rest::RestClient;
use api::websocket::WebSocketClient;
use config::Config;
use order_manager::OrderManager;
use risk_manager::RiskManager;
use strategy::grid::GridStrategy;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // 設置日誌系統
    setup_logger();
    info!("Starting BP Quant Trading Bot...");
    
    // Load configuration
    let config = Config::from_file("config.toml")?;
    info!("Configuration loaded successfully. Trading symbol: {}", config.symbol);

    // Initialize API clients
    let rest_client = RestClient::new(&config);
    let ws_client = WebSocketClient::new(&config);
    info!("API clients initialized");

    // Initialize order manager
    let order_manager = Arc::new(Mutex::new(OrderManager::new(rest_client.clone())));
    info!("Order manager initialized");

    // Initialize risk manager
    let risk_manager = Arc::new(Mutex::new(RiskManager::new(&config, order_manager.clone())));
    info!("Risk manager initialized");

    // Initialize grid strategy
    let mut strategy = GridStrategy::new(&config, order_manager.clone());
    info!("Grid strategy initialized");

    // Channel for communication between WebSocket and Strategy
    let (tx, _) = broadcast::channel(100);
    info!("Communication channels established");

    // Start WebSocket connection
    let ws_handle = tokio::spawn({
        let tx = tx.clone();
        let risk_manager = Arc::clone(&risk_manager);
        async move {
            // WebSocket客戶端現在會在內部處理重連，這裡只需要調用一次connect方法
            match ws_client.connect(tx.clone()).await {
                Ok(_) => {
                    // 這裡實際上永遠不會到達，因為connect方法現在會無限循環
                    warn!("WebSocket connection terminated unexpectedly");
                }
                Err(e) => {
                    let error_msg = format!("Critical WebSocket error - could not start connection: {}", e);
                    error!("{}", error_msg);
                    
                    // 處理嚴重錯誤
                    let should_cancel_orders = {
                        let mut risk_mgr = risk_manager.lock().await;
                        risk_mgr.record_error(&error_msg)
                    };
                    
                    if should_cancel_orders {
                        let result = {
                            let mgr = risk_manager.lock().await;
                            mgr.cancel_all_orders().await
                        };
                        if let Err(cancel_err) = result {
                            error!("Error cancelling orders: {}", cancel_err);
                        }
                    }
                }
            }
        }
    });

    // Start strategy
    let strategy_handle = tokio::spawn({
        let tx = tx.clone();
        let risk_manager = Arc::clone(&risk_manager);
        async move {
            loop {
                // 每次迭代獲取新的接收器
                let rx = tx.subscribe();
                match strategy.run(rx).await {
                    Ok(_) => {
                        warn!("Strategy stopped, restarting...");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    Err(e) => {
                        let error_msg = format!("Strategy error: {}", e);
                        error!("{}", error_msg);

                        // 先獲取鎖，處理完邏輯後立即釋放
                        let should_cancel_orders = {
                            let mut risk_mgr = risk_manager.lock().await;
                            risk_mgr.record_error(&error_msg)
                        };

                        // 如果需要取消所有訂單，在鎖外調用
                        if should_cancel_orders {
                            // 使用正確的生命週期處理
                            let result = {
                                let mgr = risk_manager.lock().await;
                                mgr.cancel_all_orders().await
                            };
                            if let Err(cancel_err) = result {
                                error!("Error cancelling orders: {}", cancel_err);
                            }
                        }
                    }
                }
            }
        }
    });

    info!("All systems started successfully");
    
    // Wait for completion
    tokio::try_join!(ws_handle, strategy_handle)?;

    Ok(())
}

// 設置日誌記錄器
fn setup_logger() {
    let mut builder = Builder::new();
    builder.format(|buf, record| {
        writeln!(
            buf,
            "{} [{}] - {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            record.level(),
            record.args()
        )
    });
    
    // 根據環境變量設置日誌級別，預設為 Info
    if let Ok(var) = std::env::var("RUST_LOG") {
        builder.parse_filters(&var);
    } else {
        builder.filter(None, log::LevelFilter::Info);
    }
    
    // 初始化日誌記錄器
    builder.init();
}
