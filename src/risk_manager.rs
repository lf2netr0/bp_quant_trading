// risk_manager.rs - Manages risk for the trading bot

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::config::Config;
use crate::order_manager::OrderManager;

pub struct RiskManager {
    config: Config,
    order_manager: Arc<Mutex<OrderManager>>,
    error_count: usize,
    last_error_time: Instant,
    panic_mode: bool,
}

impl RiskManager {
    pub fn new(config: &Config, order_manager: Arc<Mutex<OrderManager>>) -> Self {
        Self {
            config: config.clone(),
            order_manager,
            error_count: 0,
            last_error_time: Instant::now(),
            panic_mode: false,
        }
    }

    // Record an error and check if we should enter panic mode
    pub fn record_error(&mut self, error: &str) -> bool {
        println!("Error: {}", error);

        self.error_count += 1;

        // Reset error count if last error was more than 5 minutes ago
        if self.last_error_time.elapsed() > Duration::from_secs(300) {
            self.error_count = 1;
        }

        self.last_error_time = Instant::now();

        // Enter panic mode if we have too many errors
        if self.error_count >= 10 {
            self.panic_mode = true;
            println!("PANIC MODE ACTIVATED: Too many errors");
            return true;
        }

        false
    }

    // Check if we're in panic mode
    pub fn is_panic_mode(&self) -> bool {
        self.panic_mode
    }

    // Reset panic mode
    pub fn reset_panic_mode(&mut self) {
        self.panic_mode = false;
        self.error_count = 0;
    }

    // Cancel all orders in panic mode
    pub async fn cancel_all_orders(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        // 先獲取所有需要取消的訂單資訊
        let active_orders: Vec<(String, String)> = {
            let order_manager = self.order_manager.lock().await;
            order_manager
                .get_active_orders(&self.config.symbol)
                .iter()
                .map(|o| (o.symbol.clone(), o.id.clone()))
                .collect()
        };

        let mut success_count = 0;
        let mut error_count = 0;

        // 然後在迴圈中逐一取消訂單，不跨越await持有MutexGuard
        for (symbol, order_id) in active_orders {
            // 每次在單獨的作用域獲取鎖並立即釋放
            let result = {
                // 在此作用域中獲取鎖 - 移除 mut 關鍵字，因為我們不需要修改 order_manager
                let mut order_manager = self.order_manager.lock().await;
                // 調用 cancel_order 方法，它現在會處理"Order not found"的情況
                order_manager.cancel_order(&symbol, &order_id).await
            };

            match result {
                Ok(_) => {
                    println!("Cancelled order: {}", order_id);
                    success_count += 1;
                },
                Err(e) => {
                    println!("Failed to cancel order {}: {}", order_id, e);
                    error_count += 1;
                }
            }
        }

        println!("Order cancellation results: {} successful, {} failed", success_count, error_count);
        
        if error_count > 0 {
            eprintln!("Warning: Some orders could not be cancelled");
        }

        Ok(())
    }
}
