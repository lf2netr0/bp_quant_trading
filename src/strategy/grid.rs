// strategy/grid.rs - Grid trading strategy implementation

use crate::api::rest::Order;
use crate::api::websocket;
use crate::api::websocket::MarketEvent;
use crate::config::Config;
use crate::order_manager::OrderManager;
use std::cell::Cell;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

pub struct GridStrategy {
    config: Config,
    order_manager: Arc<Mutex<OrderManager>>,
    grid_prices: Vec<f64>,
    reference_price: f64,
    last_update: std::time::Instant,
    update_interval: Duration,
    total_funds: f64,
    per_grid_quantity: f64,
    last_price_cache: Cell<f64>,
    last_index_cache: Cell<usize>,
}

impl GridStrategy {
    pub fn new(config: &Config, order_manager: Arc<Mutex<OrderManager>>) -> Self {
        Self {
            config: config.clone(),
            order_manager,
            grid_prices: Vec::new(),
            reference_price: 0.0,
            last_update: std::time::Instant::now(),
            update_interval: Duration::from_millis(100), // 100ms update interval
            total_funds: 0.0,
            per_grid_quantity: 0.0,
            last_price_cache: Cell::new(0.0),
            last_index_cache: Cell::new(0),
        }
    }

    pub async fn run(
        &mut self,
        mut rx: broadcast::Receiver<MarketEvent>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Wait for initial market data
        while self.reference_price == 0.0 {
            match rx.recv().await {
                Ok(event) => self.process_market_event(event).await?,
                Err(broadcast::error::RecvError::Closed) => return Err("Channel closed".into()),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }

        println!("Initializing grid strategy...");
        println!("Reference price: {}", self.reference_price);
        self.calculate_total_funds().await?;
        println!("Total funds: {}", self.total_funds);

        self.initialize_grid()?;
        self.calculate_per_grid_quantity();
        println!("Per grid quantity: {}", self.per_grid_quantity);

        self.place_initial_orders().await?;
        println!("Initial orders placed.");

        let mut update_interval = interval(self.update_interval);

        loop {
            tokio::select! {
                _ = update_interval.tick() => {
                    if self.last_update.elapsed() > Duration::from_secs(1) {
                        self.update_orders().await?;
                        self.last_update = std::time::Instant::now();
                    }
                }
                event = rx.recv() => {
                    match event {
                        Ok(e) => {
                            self.process_market_event(e).await?;
                        },
                        Err(broadcast::error::RecvError::Closed) => {
                            break;
                        },
                        Err(broadcast::error::RecvError::Lagged(missed)) => {
                            println!("Broadcast channel lagged, missed {} messages", missed);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn calculate_total_funds(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let price_str = format!("{:.2}", self.reference_price);

        let order_manager = self.order_manager.lock().await;
        let max_quantity_bid = order_manager
            .rest_client
            .get_max_order_quantity(&self.config.symbol, "Bid", &price_str)
            .await?;

        let max_quantity_ask = order_manager
            .rest_client
            .get_max_order_quantity(&self.config.symbol, "Ask", &price_str)
            .await?;

        let bid_qty = max_quantity_bid.max_order_quantity.parse::<f64>().unwrap_or(0.0);
        let ask_qty = max_quantity_ask.max_order_quantity.parse::<f64>().unwrap_or(0.0);

        self.total_funds = bid_qty.min(ask_qty);

        Ok(())
    }

    fn calculate_per_grid_quantity(&mut self) {
        let safety_factor = 0.9;
        self.per_grid_quantity = (self.total_funds / self.config.grid_levels as f64) * safety_factor;

        self.per_grid_quantity = self.per_grid_quantity.max(self.config.min_quantity);

        let quantity_precision = 10_f64.powi(self.config.quantity_precision as i32);
        self.per_grid_quantity = (self.per_grid_quantity * quantity_precision).round() / quantity_precision;
    }

    async fn process_market_event(
        &mut self,
        event: MarketEvent,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match event {
            MarketEvent::BookTicker(ticker) => {
                let order_manager = self.order_manager.lock().await;
                order_manager.update_best_prices(&ticker);

                if self.reference_price == 0.0 {
                    let bid = ticker.inside_bid_price.parse::<f64>().unwrap_or_default();
                    let ask = ticker.inside_ask_price.parse::<f64>().unwrap_or_default();
                    self.reference_price = (bid + ask) / 2.0;
                }
            }
            MarketEvent::Trade(trade) => {
                let price = trade.price.parse::<f64>().unwrap_or_default();
                if (price - self.reference_price).abs() / self.reference_price > 0.02 {
                    self.reference_price = price;
                    self.initialize_grid()?;
                }
            }
            MarketEvent::OrderUpdate(order_update) => {
                if order_update.event_type == "orderFill" {
                    let mut order_manager = self.order_manager.lock().await;
                    order_manager.handle_order_fill(&order_update).await?;
                    drop(order_manager);
                    self.handle_fill(&order_update).await?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn initialize_grid(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let upper_bound = self.reference_price * (1.0 + self.config.grid_upper_bound);
        let lower_bound = self.reference_price * (1.0 + self.config.grid_lower_bound);
        let grid_count = self.config.grid_levels;

        let price_step = (upper_bound - lower_bound) / (grid_count as f64 - 1.0);
        
        // 使用價格精度做四捨五入
        let price_precision = 10_f64.powi(self.config.price_precision as i32);
        
        self.grid_prices = (0..grid_count)
            .map(|i| {
                let price = lower_bound + (i as f64) * price_step;
                // 對價格進行四捨五入，保證精度
                (price * price_precision).round() / price_precision
            })
            .collect();

        self.last_price_cache.set(0.0);
        self.last_index_cache.set(0);

        println!(
            "Grid initialized with {} levels, price range: {} - {}",
            grid_count, 
            self.grid_prices.first().unwrap_or(&lower_bound), 
            self.grid_prices.last().unwrap_or(&upper_bound)
        );

        Ok(())
    }

    async fn place_initial_orders(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        // 先獲取當前市場價格
        let order_manager = self.order_manager.lock().await;
        let (best_bid, best_ask) = order_manager
            .get_best_prices(&self.config.symbol)
            .unwrap_or((self.reference_price * 0.99, self.reference_price * 1.01));
        let current_market_price = (best_bid + best_ask) / 2.0;
        
        let current_price_index = self.find_closest_grid_index(current_market_price);
        println!("Current price index: {}, Market price: {}", current_price_index, current_market_price);
        
        // 重要修改: 使用 clone() 創建擁有的數據副本而不是引用
        let bid_orders: Vec<Order> = order_manager
            .get_active_orders_by_side(&self.config.symbol, "Bid")
            .into_iter()
            .map(|order| order.clone())
            .collect();
        
        let ask_orders: Vec<Order> = order_manager
            .get_active_orders_by_side(&self.config.symbol, "Ask")
            .into_iter()
            .map(|order| order.clone())
            .collect();
        
        drop(order_manager);  // 現在可以安全地丟棄 MutexGuard

        // 如果沒有任何活躍訂單，創建初始訂單
        if bid_orders.is_empty() && ask_orders.is_empty() {
            println!("No active orders found, placing initial grid orders");
            
            // 遍歷所有格子
            for (i, &price) in self.grid_prices.iter().enumerate() {
                // 決定訂單方向：如果格子價格低於市場價格，掛買單；否則掛賣單
                let side = if price < current_market_price {
                    "Bid"  // 買單
                } else if price > current_market_price {
                    "Ask"  // 賣單
                } else {
                    continue;  // 與市場價格相同，跳過這個價格
                };
                
                // 只掛離市場價格最近的訂單（3個買單和3個賣單）
                if side == "Bid" && i < current_price_index && current_price_index - i <= 3 {
                    println!("Placing bid order at price: {}", price);
                    
                    let mut order_manager = self.order_manager.lock().await;
                    order_manager
                        .place_order(&self.config.symbol, side, price, self.per_grid_quantity)
                        .await?;
                }
                else if side == "Ask" && i > current_price_index && i - current_price_index <= 3 {
                    println!("Placing ask order at price: {}", price);
                    
                    let mut order_manager = self.order_manager.lock().await;
                    order_manager
                        .place_order(&self.config.symbol, side, price, self.per_grid_quantity)
                        .await?;
                }
            }
        } else {
            println!("Found existing orders, checking for missing grid levels");
            self.update_orders().await?;
        }

        Ok(())
    }

    async fn update_orders(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        // 獲取當前市場價格
        let order_manager = self.order_manager.lock().await;
        let (best_bid, best_ask) = order_manager
            .get_best_prices(&self.config.symbol)
            .unwrap_or((self.reference_price * 0.99, self.reference_price * 1.01));
        let current_market_price = (best_bid + best_ask) / 2.0;
        
        // 重要修改: 使用 clone() 創建擁有的數據副本而不是引用
        let active_bid_orders: Vec<Order> = order_manager
            .get_active_orders_by_side(&self.config.symbol, "Bid")
            .into_iter()
            .map(|order| order.clone())
            .collect();
        
        let active_ask_orders: Vec<Order> = order_manager
            .get_active_orders_by_side(&self.config.symbol, "Ask")
            .into_iter()
            .map(|order| order.clone())
            .collect();
        
        drop(order_manager);  // 現在可以安全地丟棄 MutexGuard
        
        // 找到最接近當前市場價格的網格點
        let current_price_index = self.find_closest_grid_index(current_market_price);
        
        
        
        // 計算目標買單和賣單價格
        let target_bid_prices: Vec<f64> = (1..=3)
            .filter_map(|i| {
                if current_price_index >= i {
                    Some(self.grid_prices[current_price_index - i])
                } else {
                    None
                }
            })
            .collect();

        let target_ask_prices: Vec<f64> = (1..=3)
            .filter_map(|i| {
                if current_price_index + i < self.grid_prices.len() {
                    Some(self.grid_prices[current_price_index + i])
                } else {
                    None
                }
            })
            .collect();
        
        // 步驟2: 掛出缺少的買單
        for &price in &target_bid_prices {
            let bid_exists = active_bid_orders.iter().any(|order| {
                order.price.as_ref().map_or(false, |p| {
                    (p.parse::<f64>().unwrap_or_default() - price).abs() < 0.0001
                })
            });
            
            if !bid_exists {
                println!("添加缺少的買單，價格: {}", price);
                let mut order_manager = self.order_manager.lock().await;
                if let Err(e) = order_manager
                    .place_order(&self.config.symbol, "Bid", price, self.per_grid_quantity)
                    .await
                {
                    println!("無法下買單，價格 {}: {}", price, e);
                    continue;
                }
            }
        }
        
        // 步驟2: 掛出缺少的賣單
        for &price in &target_ask_prices {
            let ask_exists = active_ask_orders.iter().any(|order| {
                order.price.as_ref().map_or(false, |p| {
                    (p.parse::<f64>().unwrap_or_default() - price).abs() < 0.0001
                })
            });
            
            if !ask_exists {
                println!("添加缺少的賣單，價格: {}", price);
                let mut order_manager = self.order_manager.lock().await;
                if let Err(e) = order_manager
                    .place_order(&self.config.symbol, "Ask", price, self.per_grid_quantity)
                    .await
                {
                    println!("無法下賣單，價格 {}: {}", price, e);
                    continue;
                }
            }
        }
        // 步驟1: 先取消不是當前最近3個網格的買單
        for order in &active_bid_orders {
            if let Some(price_str) = &order.price {
                if let Ok(price) = price_str.parse::<f64>() {
                    // 找出這個價格最接近的網格點
                    let order_price_index = self.find_closest_grid_index(price);
                    
                    // 檢查這個網格點是否在目標買單網格範圍內
                    let is_target = (1..=3)
                        .filter_map(|i| {
                            if current_price_index >= i {
                                Some(current_price_index - i)
                            } else {
                                None
                            }
                        })
                        .any(|idx| idx == order_price_index);
                        
                    if !is_target {
                        println!("取消非最近網格的買單，價格: {} (索引: {}), 當前市場價格索引: {}", 
                                 price, order_price_index, current_price_index);
                        let mut order_manager = self.order_manager.lock().await;
                        if let Err(e) = order_manager.cancel_order(&order.symbol, &order.id).await {
                            println!("無法取消買單 {}: {}", order.id, e);
                            continue;
                        }
                    }
                }
            }
        }
        
        // 步驟1: 先取消不是當前最近3個網格的賣單
        for order in &active_ask_orders {
            if let Some(price_str) = &order.price {
                if let Ok(price) = price_str.parse::<f64>() {
                    // 找出這個價格最接近的網格點
                    let order_price_index = self.find_closest_grid_index(price);
                    
                    // 檢查這個網格點是否在目標賣單網格範圍內
                    let is_target = (1..=3)
                        .filter_map(|i| {
                            if current_price_index + i < self.grid_prices.len() {
                                Some(current_price_index + i)
                            } else {
                                None
                            }
                        })
                        .any(|idx| idx == order_price_index);
                        
                    if !is_target {
                        println!("取消非最近網格的賣單，價格: {} (索引: {}), 當前市場價格索引: {}", 
                                 price, order_price_index, current_price_index);
                        let mut order_manager = self.order_manager.lock().await;
                        if let Err(e) = order_manager.cancel_order(&order.symbol, &order.id).await {
                            println!("無法取消賣單 {}: {}", order.id, e);
                            continue;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_fill(
        &mut self,
        order_update: &websocket::OrderUpdate,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // 獲取成交價格
        let filled_price = if let Some(price_str) = &order_update.last_filled_price {
            price_str.parse::<f64>().unwrap_or(self.reference_price)
        } else {
            self.reference_price
        };
        
        // 獲取成交方向
        let filled_side = &order_update.side;
        
        // 獲取當前市場價格
        let order_manager = self.order_manager.lock().await;
        let (best_bid, best_ask) = order_manager
            .get_best_prices(&self.config.symbol)
            .unwrap_or((self.reference_price * 0.99, self.reference_price * 1.01));
        drop(order_manager);
        
        let current_market_price = (best_bid + best_ask) / 2.0;
        
        // 找到在網格中適合的新訂單價格
        let current_price_index = self.find_closest_grid_index(filled_price);
        
        // 如果成交是買單，我們需要在合適的價格放置賣單
        // 如果成交是賣單，我們需要在合適的價格放置買單
        let (new_price, new_side) = if filled_side == "Bid" {
            // 買單成交，應該在高於成交價格的地方放賣單
            // 但前提是這個價格高於市場價格
            let candidate_index = current_price_index + 1;
            if candidate_index < self.grid_prices.len() {
                let candidate_price = self.grid_prices[candidate_index];
                if candidate_price > current_market_price {
                    (candidate_price, "Ask")
                } else {
                    // 如果候選價格不高於市場價格，那我們在當前市場價格上方的第一個網格點放賣單
                    let market_idx = self.find_closest_grid_index(current_market_price);
                    if market_idx + 1 < self.grid_prices.len() {
                        (self.grid_prices[market_idx + 1], "Ask")
                    } else {
                        (current_market_price * 1.01, "Ask") // 備用方案
                    }
                }
            } else {
                (filled_price * 1.01, "Ask") // 備用方案
            }
        } else {
            // 賣單成交，應該在低於成交價格的地方放買單
            // 但前提是這個價格低於市場價格
            let candidate_index = if current_price_index > 0 { current_price_index - 1 } else { 0 };
            if candidate_index < current_price_index {
                let candidate_price = self.grid_prices[candidate_index];
                if candidate_price < current_market_price {
                    (candidate_price, "Bid")
                } else {
                    // 如果候選價格不低於市場價格，那我們在當前市場價格下方的第一個網格點放買單
                    let market_idx = self.find_closest_grid_index(current_market_price);
                    if market_idx > 0 {
                        (self.grid_prices[market_idx - 1], "Bid")
                    } else {
                        (current_market_price * 0.99, "Bid") // 備用方案
                    }
                }
            } else {
                (filled_price * 0.99, "Bid") // 備用方案
            }
        };
        
        // 放置新訂單
        let mut order_manager = self.order_manager.lock().await;
        order_manager
            .place_order(&self.config.symbol, new_side, new_price, self.per_grid_quantity)
            .await?;
        
        println!(
            "訂單成交 {} 在價格 {}，已放置新的 {} 訂單在價格 {}",
            filled_side, filled_price, new_side, new_price
        );
        
        Ok(())
    }

    fn find_closest_grid_index(&self, price: f64) -> usize {
        if (self.last_price_cache.get() - price).abs() < f64::EPSILON {
            return self.last_index_cache.get();
        }
        
        let result = match self.grid_prices.binary_search_by(|&x| {
            if x < price {
                std::cmp::Ordering::Less
            } else if x > price {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(index) => index,
            Err(index) => {
                if index == 0 {
                    0
                } else if index >= self.grid_prices.len() {
                    self.grid_prices.len() - 1
                } else {
                    let prev_diff = (price - self.grid_prices[index - 1]).abs();
                    let next_diff = (self.grid_prices[index] - price).abs();
                    if prev_diff < next_diff {
                        index - 1
                    } else {
                        index
                    }
                }
            }
        };
        
        self.last_price_cache.set(price);
        self.last_index_cache.set(result);
        
        result
    }
}
