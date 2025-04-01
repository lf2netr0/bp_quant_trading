// api/websocket.rs - WebSocket client for Backpack Exchange

use crate::config::Config;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::error::Error;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

pub struct WebSocketClient {
    config: Config,
    signing_key: SigningKey,
}

impl WebSocketClient {
    pub fn new(config: &Config) -> Self {
        // Generate ED25519 keypair from the provided secret
        let secret_bytes = BASE64.decode(&config.api_secret).unwrap();
        let secret_array: [u8; 32] = secret_bytes.try_into().expect("Invalid secret key length");
        let signing_key = SigningKey::from_bytes(&secret_array);

        Self {
            config: config.clone(),
            signing_key,
        }
    }

    pub async fn connect(
        &self,
        tx: broadcast::Sender<MarketEvent>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // 指數退避重連設置
        let mut reconnect_delay = Duration::from_millis(100);
        const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
        
        // 連接 WebSocket 並持續維持連接
        loop {
            println!("Connecting to WebSocket at {}", self.config.ws_url);
            let ws_stream_result = connect_async(self.config.ws_url.clone()).await;
            
            match ws_stream_result {
                Ok((ws_stream, _)) => {
                    // 連接成功，重置重連延遲
                    println!("WebSocket connected successfully");
                    reconnect_delay = Duration::from_millis(100);
                    
                    let (mut write, mut read) = ws_stream.split();
                    
                    // 訂閱所有需要的流
                    match self.subscribe_to_streams(&mut write).await {
                        Ok(_) => println!("Successfully subscribed to all streams"),
                        Err(e) => {
                            eprintln!("Failed to subscribe to streams: {}", e);
                            tokio::time::sleep(reconnect_delay).await;
                            continue; // 嘗試重新連接
                        }
                    }
                    
                    // 設置最後接收pong的時間
                    // 根據文件，服務器會每60秒發送一次ping，我們需要回應pong
                    // 如果120秒沒有收到pong，服務器會關閉連接
                    let mut last_pong_time = Instant::now();
                    const MAX_NO_PONG_TIME: Duration = Duration::from_secs(90); // 保守一點設置為90秒，低於120秒
                    
                    // 處理接收到的訊息
                    loop {
                        tokio::select! {
                            msg_option = read.next() => {
                                match msg_option {
                                    Some(Ok(Message::Text(text))) => {
                                        // 更新最後接收消息時間
                                        match self.parse_market_event(&text) {
                                            Ok(event) => {
                                                if tx.send(event).is_err() {
                                                    eprintln!("Failed to send event through channel - receiver might be dropped");
                                                }
                                            },
                                            Err(e) => eprintln!("Failed to parse message: {}", e),
                                        }
                                    },
                                    Some(Ok(Message::Ping(ping_data))) => {
                                        // 根據文件，服務器每60秒發送一次ping，我們需要回應pong
                                        println!("Received ping from server, responding with pong");
                                        last_pong_time = Instant::now(); // 更新最後ping/pong時間
                                        if let Err(e) = write.send(Message::Pong(ping_data)).await {
                                            eprintln!("Failed to send pong: {}", e);
                                            break;
                                        }
                                    },
                                    Some(Ok(Message::Pong(_))) => {
                                        // 收到pong回應，更新時間
                                        last_pong_time = Instant::now();
                                    },
                                    Some(Ok(Message::Close(frame))) => {
                                        // 伺服器要求關閉連接
                                        // 根據文檔，這可能是服務器正在關閉，我們應該重新連接到另一個服務器
                                        if let Some(frame) = frame {
                                            println!("Server is closing connection: ({}) {}", frame.code, frame.reason);
                                        } else {
                                            println!("Server is closing connection");
                                        }
                                        println!("Will attempt to reconnect...");
                                        break;
                                    },
                                    Some(Err(e)) => {
                                        eprintln!("WebSocket error: {}", e);
                                        break;
                                    },
                                    None => {
                                        eprintln!("WebSocket stream ended");
                                        break;
                                    },
                                    _ => {} // 忽略其他消息類型
                                }
                            },
                            
                            // 定期檢查是否需要重連
                            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                                // 檢查我們是否太久沒有收到ping/pong
                                if last_pong_time.elapsed() > MAX_NO_PONG_TIME {
                                    eprintln!("No ping received from server for {} seconds, reconnecting...", 
                                              MAX_NO_PONG_TIME.as_secs());
                                    break;
                                }
                            },
                        }
                    }
                    
                    // 如果從消息處理循環中退出，等待後嘗試重新連接
                    println!("WebSocket connection disrupted, attempting to reconnect in {:?}...", reconnect_delay);
                },
                Err(e) => {
                    eprintln!("Failed to connect to WebSocket: {}", e);
                }
            }
            
            // 應用指數退避延遲
            tokio::time::sleep(reconnect_delay).await;
            reconnect_delay = std::cmp::min(reconnect_delay * 2, MAX_RECONNECT_DELAY);
        }
    }

    async fn subscribe_to_streams<S>(
        &self,
        write: &mut S,
    ) -> Result<(), Box<dyn Error + Send + Sync>>
    where
        S: SinkExt<Message> + Unpin,
        S::Error: Error + Send + Sync + 'static,
    {
        // Subscribe to book ticker for our symbol
        let book_ticker_stream = format!("bookTicker.{}", self.config.symbol);
        self.subscribe(write, &book_ticker_stream).await?;

        // Subscribe to depth for our symbol
        let depth_stream = format!("depth.{}", self.config.symbol);
        self.subscribe(write, &depth_stream).await?;

        // Subscribe to trades for our symbol
        let trade_stream = format!("trade.{}", self.config.symbol);
        self.subscribe(write, &trade_stream).await?;

        // Subscribe to private order updates
        let timestamp = self.get_timestamp();
        let window = 5000;
        let signature_string = format!(
            "instruction=subscribe&timestamp={}&window={}",
            timestamp, window
        );
        let signature = self.signing_key.sign(signature_string.as_bytes());
        let signature_b64 = BASE64.encode(signature.to_bytes());

        let private_subscription = json!({
            "method": "SUBSCRIBE",
            "params": ["account.orderUpdate"],
            "signature": [
                self.config.api_key,
                signature_b64,
                timestamp.to_string(),
                window.to_string()
            ]
        });

        write
            .send(Message::Text(private_subscription.to_string()))
            .await?;

        Ok(())
    }

    async fn subscribe<S>(
        &self,
        write: &mut S,
        stream: &str,
    ) -> Result<(), Box<dyn Error + Send + Sync>>
    where
        S: SinkExt<Message> + Unpin,
        S::Error: Error + Send + Sync + 'static,
    {
        let subscription = json!({
            "method": "SUBSCRIBE",
            "params": [stream]
        });

        write.send(Message::Text(subscription.to_string())).await?;
        Ok(())
    }

    fn parse_market_event(&self, text: &str) -> Result<MarketEvent, Box<dyn Error + Send + Sync>> {
        match serde_json::from_str::<Value>(text) {
            Ok(v) => {
                if let Some(stream) = v.get("stream").and_then(|s| s.as_str()) {
                    let data = v.get("data").ok_or("No data field in WebSocket message")?;

                    if stream.starts_with("bookTicker.") {
                        // 添加錯誤處理
                        match serde_json::from_value::<BookTicker>(data.clone()) {
                            Ok(ticker) => return Ok(MarketEvent::BookTicker(ticker)),
                            Err(e) => {
                                eprintln!("Failed to parse bookTicker: {}", e);
                                eprintln!("Raw data: {}", serde_json::to_string_pretty(data).unwrap_or_default());
                                return Err(Box::new(e));
                            }
                        }
                    } else if stream.starts_with("depth.") {
                        match serde_json::from_value::<Depth>(data.clone()) {
                            Ok(depth) => return Ok(MarketEvent::Depth(depth)),
                            Err(e) => {
                                eprintln!("Failed to parse depth: {}", e);
                                eprintln!("Raw data: {}", serde_json::to_string_pretty(data).unwrap_or_default());
                                return Err(Box::new(e));
                            }
                        }
                    } else if stream.starts_with("trade.") {
                        match serde_json::from_value::<Trade>(data.clone()) {
                            Ok(trade) => return Ok(MarketEvent::Trade(trade)),
                            Err(e) => {
                                eprintln!("Failed to parse trade: {}", e);
                                eprintln!("Raw data: {}", serde_json::to_string_pretty(data).unwrap_or_default());
                                return Err(Box::new(e));
                            }
                        }
                    } else if stream == "account.orderUpdate" {
                        match serde_json::from_value::<OrderUpdate>(data.clone()) {
                            Ok(order) => return Ok(MarketEvent::OrderUpdate(order)),
                            Err(e) => {
                                eprintln!("Failed to parse orderUpdate: {}", e);
                                eprintln!("Raw data: {}", serde_json::to_string_pretty(data).unwrap_or_default());
                                return Err(Box::new(e));
                            }
                        }
                    }
                }
                Err("Unknown event type".into())
            },
            Err(e) => {
                eprintln!("Failed to parse WebSocket message: {}", e);
                eprintln!("Raw message: {}", text);
                Err(Box::new(e))
            }
        }
    }

    // Helper method to get current timestamp in milliseconds
    fn get_timestamp(&self) -> u64 {
        let start = SystemTime::now();
        let since_the_epoch = start
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
        since_the_epoch.as_millis() as u64
    }
}

#[derive(Debug, Clone)]
pub enum MarketEvent {
    BookTicker(BookTicker),
    Depth(Depth),
    Trade(Trade),
    OrderUpdate(OrderUpdate),
}

#[derive(Debug, Deserialize, Clone)]
pub struct BookTicker {
    #[serde(rename = "e")]
    pub event_type: String,           // Event type
    #[serde(rename = "E")]
    pub event_time: u64,              // Event time in microseconds
    #[serde(rename = "s")]
    pub symbol: String,               // Symbol
    #[serde(rename = "a")]
    pub inside_ask_price: String,     // Inside ask price
    #[serde(rename = "A")]
    pub inside_ask_quantity: String,  // Inside ask quantity
    #[serde(rename = "b")]
    pub inside_bid_price: String,     // Inside bid price
    #[serde(rename = "B")]
    pub inside_bid_quantity: String,  // Inside bid quantity
    #[serde(rename = "u")]
    pub update_id: Value,             // 將 String 改為 Value 以適應不同類型
    #[serde(rename = "T")]
    pub engine_timestamp: u64,        // Engine timestamp in microseconds
}

#[derive(Debug, Deserialize, Clone)]
pub struct Depth {
    #[serde(rename = "e")]
    pub event_type: String,           // Event type
    #[serde(rename = "E")]
    pub event_time: u64,              // Event time in microseconds
    #[serde(rename = "s")]
    pub symbol: String,               // Symbol
    #[serde(rename = "a")]
    pub asks: Vec<Vec<String>>,       // Asks [price, quantity]
    #[serde(rename = "b")]
    pub bids: Vec<Vec<String>>,       // Bids [price, quantity]
    #[serde(rename = "U")]
    pub first_update_id: u64,         // First update ID in event
    #[serde(rename = "u")]
    pub last_update_id: u64,          // Last update ID in event
    #[serde(rename = "T")]
    pub engine_timestamp: u64,        // Engine timestamp in microseconds
}

#[derive(Debug, Deserialize, Clone)]
pub struct Trade {
    #[serde(rename = "e")]
    pub event_type: String,           // Event type
    #[serde(rename = "E")]
    pub event_time: u64,              // Event time in microseconds
    #[serde(rename = "s")]
    pub symbol: String,               // Symbol
    #[serde(rename = "p")]
    pub price: String,                // Price
    #[serde(rename = "q")]
    pub quantity: String,             // Quantity
    #[serde(rename = "b")]
    pub buyer_order_id: Value,        // 將 String 改為 Value 以適應不同類型
    #[serde(rename = "a")]
    pub seller_order_id: Value,       // 將 String 改為 Value 以適應不同類型
    #[serde(rename = "t")]
    pub trade_id: u64,                // Trade ID
    #[serde(rename = "T")]
    pub engine_timestamp: u64,        // Engine timestamp in microseconds
    #[serde(rename = "m")]
    pub is_buyer_maker: bool,         // Is the buyer the maker?
}

#[derive(Debug, Deserialize, Clone)]
pub struct OrderUpdate {
    #[serde(rename = "e")]
    pub event_type: String, // Event type (orderAccepted, orderFill, orderCancelled, etc.)
    #[serde(rename = "E")]
    pub event_time: u64,    // Event time in microseconds
    #[serde(rename = "s")]
    pub symbol: String,     // Symbol
    #[serde(rename = "c")]
    pub client_order_id: Option<Value>, // 將 Option<u32> 改為 Option<Value>
    #[serde(rename = "S")]
    pub side: String,       // Side (Bid/Ask)
    #[serde(rename = "o")]
    pub order_type: String, // Order type
    #[serde(rename = "f")]
    pub time_in_force: String, // Time in force
    #[serde(rename = "q")]
    pub quantity: Option<String>, // Quantity (optional)
    #[serde(rename = "Q")]
    pub quote_quantity: Option<String>, // Quote quantity (optional)
    #[serde(rename = "p")]
    pub price: Option<String>, // Price (optional)
    #[serde(rename = "P")]
    pub trigger_price: Option<String>, // Trigger price (optional)
    #[serde(rename = "B")]
    pub trigger_by: Option<String>, // Trigger by (optional)
    #[serde(rename = "a")]
    pub take_profit_price: Option<String>, // Take profit trigger price (optional)
    #[serde(rename = "b")]
    pub stop_loss_price: Option<String>, // Stop loss trigger price (optional)
    #[serde(rename = "d")]
    pub take_profit_trigger_by: Option<String>, // Take profit trigger by (optional)
    #[serde(rename = "g")]
    pub stop_loss_trigger_by: Option<String>, // Stop loss trigger by (optional)
    #[serde(rename = "Y")]
    pub trigger_quantity: Option<String>, // Trigger quantity (optional)
    #[serde(rename = "X")]
    pub order_state: String, // Order state
    #[serde(rename = "R")]
    pub order_expiry_reason: Option<String>, // Order expiry reason (optional)
    #[serde(rename = "i")]
    pub order_id: String,   // Order ID
    #[serde(rename = "t")]
    pub trade_id: Option<u64>, // Trade ID (optional)
    #[serde(rename = "l")]
    pub fill_quantity: Option<String>, // Fill quantity (optional)
    #[serde(rename = "z")]
    pub executed_quantity: Option<String>, // Executed quantity (optional)
    #[serde(rename = "Z")]
    pub executed_quote_quantity: Option<String>, // Executed quote quantity (optional)
    #[serde(rename = "L")]
    pub last_filled_price: Option<String>, // Last filled price (optional)
    #[serde(rename = "m")]
    pub is_maker: Option<bool>, // Whether the order was maker (optional)
    #[serde(rename = "n")]
    pub fee: Option<String>, // Fee (optional)
    #[serde(rename = "N")]
    pub fee_symbol: Option<String>, // Fee symbol (optional)
    #[serde(rename = "V")]
    pub self_trade_prevention: Option<String>, // Self trade prevention (optional)
    #[serde(rename = "T")]
    pub engine_timestamp: u64, // Engine timestamp in microseconds
    #[serde(rename = "O")]
    pub origin: Option<String>, // Origin of the update (optional)
    #[serde(rename = "I")]
    pub related_order_id: Option<String>, // Related order ID (optional)
}
