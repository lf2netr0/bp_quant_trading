// order_manager.rs - Manages orders for the trading strategy

use rand::Rng;
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex}; // 添加 Rng 特徵引用

use crate::api::rest::{Order, PlaceOrderRequest, RestClient};
use crate::api::websocket;

pub struct OrderManager {
    pub rest_client: RestClient,
    pub active_orders: HashMap<String, Order>,
    pub last_prices: Arc<Mutex<HashMap<String, f64>>>, // Keep track of last prices
}

impl OrderManager {
    pub fn new(rest_client: RestClient) -> Self {
        Self {
            rest_client,
            active_orders: HashMap::new(),
            last_prices: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // Place a new order
    pub async fn place_order(
        &mut self,
        symbol: &str,
        side: &str,
        price: f64,
        quantity: f64,
    ) -> Result<Order, Box<dyn Error + Send + Sync>> {
        // Format price and quantity with correct precision
        let price_str = format!("{:}", price); // 2 decimal places
        let quantity_str = format!("{:}", quantity); // 2 decimal places

        // Create client ID for the order
        let client_id = rand::thread_rng().gen::<u32>();

        let order_request = PlaceOrderRequest {
            symbol: symbol.to_string(),
            side: side.to_string(),
            order_type: "Limit".to_string(),
            time_in_force: "GTC".to_string(), // Good Till Cancelled
            price: Some(price_str),
            quantity: quantity_str,
            client_id: Some(client_id),
            auto_borrow: Some(true),
            auto_borrow_repay: Some(true),
        };
        println!("Placing order: {:?}", order_request);
        // Try to place the order
        let order = self.rest_client.place_order(order_request).await?;

        // Store the active order
        self.active_orders.insert(order.id.clone(), order.clone());

        Ok(order)
    }

    // Cancel an existing order
    pub async fn cancel_order(
        &mut self,
        symbol: &str,
        order_id: &str,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Try to cancel the order
        match self.rest_client.cancel_order(symbol, order_id).await {
            Ok(_) => {
                // Remove from active orders
                self.active_orders.remove(order_id);
                Ok(())
            }
            Err(e) => {
                // 檢查錯誤是否為"Order not found"
                let error_str = e.to_string();
                if error_str.contains("Order not found") {
                    println!("Order {} not found on exchange, removing from local tracking", order_id);
                    // 如果訂單已經不存在，仍然從我們的活躍訂單中移除它
                    self.active_orders.remove(order_id);
                    Ok(())
                } else {
                    // 其他錯誤則傳回
                    Err(e)
                }
            }
        }
    }

    // Update an order (cancel and replace)
    pub async fn update_order(
        &mut self,
        symbol: &str,
        order_id: &str,
        new_price: f64,
        new_quantity: f64,
    ) -> Result<Order, Box<dyn Error + Send + Sync>> {
        // Cancel the existing order
        self.cancel_order(symbol, order_id).await?;

        // Get the side of the order
        let side = if let Some(order) = self.active_orders.get(order_id) {
            order.side.clone()
        } else {
            return Err("Order not found".into());
        };

        // Place a new order with updated price and quantity
        let new_order = self
            .place_order(symbol, &side, new_price, new_quantity)
            .await?;

        Ok(new_order)
    }

    // Handle order fill event
    pub async fn handle_order_fill(
        &mut self,
        order_update: &websocket::OrderUpdate,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if order_update.event_type == "orderFill" {
            // Remove the filled order from active orders
            self.active_orders.remove(&order_update.order_id);

            // Update last price
            if let Some(price_str) = &order_update.last_filled_price {
                if let Ok(price) = price_str.parse::<f64>() {
                    let mut last_prices = self.last_prices.lock().unwrap();
                    last_prices.insert(order_update.symbol.clone(), price);
                }
            }
        }

        Ok(())
    }

    // Get current best bid/ask prices
    pub fn get_best_prices(&self, symbol: &str) -> Option<(f64, f64)> {
        // Get from last_prices map
        let last_prices = self.last_prices.lock().unwrap();
        last_prices
            .get(symbol)
            .map(|&price| (price * 0.999, price * 1.001)) // Default spread
    }

    // Update best prices from book ticker
    pub fn update_best_prices(&self, ticker: &websocket::BookTicker) {
        let bid = ticker.inside_bid_price.parse::<f64>().unwrap_or_default();
        let ask = ticker.inside_ask_price.parse::<f64>().unwrap_or_default();

        let mid_price = (bid + ask) / 2.0;

        let mut last_prices = self.last_prices.lock().unwrap();
        last_prices.insert(ticker.symbol.clone(), mid_price);
    }

    // Get active orders count
    pub fn get_active_orders_count(&self, symbol: &str, side: Option<&str>) -> usize {
        match side {
            Some(s) => self
                .active_orders
                .values()
                .filter(|o| o.symbol == symbol && o.side == s)
                .count(),
            None => self
                .active_orders
                .values()
                .filter(|o| o.symbol == symbol)
                .count(),
        }
    }

    // Get all active orders
    pub fn get_active_orders(&self, symbol: &str) -> Vec<&Order> {
        self.active_orders
            .values()
            .filter(|o| o.symbol == symbol)
            .collect()
    }

    // Get active orders by side
    pub fn get_active_orders_by_side(&self, symbol: &str, side: &str) -> Vec<&Order> {
        self.active_orders
            .values()
            .filter(|o| o.symbol == symbol && o.side == side)
            .collect()
    }
}
