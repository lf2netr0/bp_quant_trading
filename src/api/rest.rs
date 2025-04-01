// api/rest.rs - REST API client for Backpack Exchange

use crate::config::Config;
use base64::{engine::general_purpose, Engine as _};
use ed25519_dalek::{Signer, SigningKey};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct RestClient {
    client: Client,
    config: Config,
    signing_key: SigningKey,
}

impl RestClient {
    pub fn new(config: &Config) -> Self {
        // Generate ED25519 keypair from the provided secret
        let secret_bytes = general_purpose::STANDARD.decode(&config.api_secret).unwrap();
        let secret_array: [u8; 32] = secret_bytes.try_into().expect("Invalid secret key length");
        let signing_key = SigningKey::from_bytes(&secret_array);

        Self {
            client: Client::new(),
            config: config.clone(),
            signing_key,
        }
    }

    pub async fn get_account(&self) -> Result<Account, Box<dyn Error + Send + Sync>> {
        let endpoint = "/api/v1/account";
        let method = "GET";
        let params = None;

        let (timestamp, window, signature) = self.sign(endpoint, method, &params)?;

        let response = self.client
            .get(format!("{}{}", self.config.base_url, endpoint))
            .header("X-API-KEY", &self.config.api_key)
            .header("X-TIMESTAMP", &timestamp)
            .header("X-WINDOW", &window)
            .header("X-SIGNATURE", signature)
            .send()
            .await?;

        let account: Account = response.json().await?;
        Ok(account)
    }

    pub async fn get_max_order_quantity(
        &self,
        symbol: &str,
        side: &str,
        price: &str,
    ) -> Result<MaxOrderQuantity, Box<dyn Error + Send + Sync>> {
        let endpoint = "/api/v1/account/limits/order";
        let method = "GET";

        // 創建參數對象
        let mut params_map = BTreeMap::new();
        params_map.insert("autoBorrow", "true");
        params_map.insert("autoBorrowRepay", "true");
        params_map.insert("autoLendRedeem", "true");
        params_map.insert("side", side);
        params_map.insert("symbol", symbol);

        let params = Some(serde_json::to_value(params_map)?);

        let (timestamp, window, signature) = self.sign(endpoint, method, &params)?;

        // 將參數轉換為查詢參數陣列
        let query_params = [
            ("autoBorrow", "true"),
            ("autoBorrowRepay", "true"),
            ("autoLendRedeem", "true"),
            ("side", side),
            ("symbol", symbol),
        ];

        // 使用 reqwest 的 query 方法來正確編碼查詢參數
        let response = self.client
            .get(format!("{}{}", self.config.base_url, endpoint))
            .query(&query_params)
            .header("X-API-KEY", &self.config.api_key)
            .header("X-TIMESTAMP", &timestamp)
            .header("X-WINDOW", &window)
            .header("X-SIGNATURE", &signature)
            .send()
            .await?;

        let max_quantity: MaxOrderQuantity = response.json().await?;
        Ok(max_quantity)
    }

    pub async fn place_order(
        &self,
        order: PlaceOrderRequest,
    ) -> Result<Order, Box<dyn Error + Send + Sync>> {
        let endpoint = "/api/v1/order";
        let method = "POST";

        // 將訂單請求轉換為 JSON Value
        let params = Some(serde_json::to_value(&order)?);

        let (timestamp, window, signature) = self.sign(endpoint, method, &params)?;

        let response = self.client
            .post(format!("{}{}", self.config.base_url, endpoint))
            .header("X-API-KEY", &self.config.api_key)
            .header("X-TIMESTAMP", &timestamp)
            .header("X-WINDOW", &window)
            .header("X-SIGNATURE", &signature)
            .json(&order)
            .send()
            .await?;

        match response.status() {
            reqwest::StatusCode::OK => {
                let order_response: Order = response.json().await?;
                Ok(order_response)
            }
            _ => {
                let error_text = response.text().await?;
                Err(format!("Error placing order: {}", error_text).into())
            }
        }
    }

    pub async fn cancel_order(
        &self,
        symbol: &str,
        order_id: &str,
    ) -> Result<Order, Box<dyn Error + Send + Sync>> {
        let endpoint = "/api/v1/order";
        let method = "DELETE";

        let cancel_request = CancelOrderRequest {
            symbol: symbol.to_string(),
            order_id: order_id.to_string(),
        };

        // 將取消訂單請求轉換為 JSON Value
        let params = Some(serde_json::to_value(&cancel_request)?);

        let (timestamp, window, signature) = self.sign(endpoint, method, &params)?;

        let response = self.client
            .delete(format!("{}{}", self.config.base_url, endpoint))
            .header("X-API-KEY", &self.config.api_key)
            .header("X-TIMESTAMP", &timestamp)
            .header("X-WINDOW", &window)
            .header("X-SIGNATURE", &signature)
            .json(&cancel_request)
            .send()
            .await?;

        match response.status() {
            reqwest::StatusCode::OK => {
                let order_response: Order = response.json().await?;
                Ok(order_response)
            }
            _ => {
                let error_text = response.text().await?;
                Err(format!("Error cancelling order: {}", error_text).into())
            }
        }
    }

    // 新的統一簽名方法
    fn sign(&self, endpoint: &str, method: &str, params: &Option<Value>) -> Result<(String, String, String), Box<dyn Error + Send + Sync>> {
        let timestamp = self.get_timestamp();
        let window = 5000; // 使用一個固定的窗口值
        let instruction_type = self.get_instruction_type(endpoint, method);

        // 開始構建簽名字符串
        let mut sign_parts = Vec::new();
        sign_parts.push(format!("instruction={}", instruction_type));

        // 添加請求參數 (按字母順序排序)
        if let Some(params_value) = params {
            let mut params_map = BTreeMap::new();

            if let Some(params_obj) = params_value.as_object() {
                for (k, v) in params_obj {
                    let v_str = if v.is_string() {
                        v.as_str().unwrap().to_string()
                    } else {
                        v.to_string().trim_matches('"').to_string()
                    };
                    params_map.insert(k.clone(), v_str);
                }
            }

            for (k, v) in params_map {
                sign_parts.push(format!("{}={}", k, v));
            }
        }

        // 添加時間戳和窗口
        sign_parts.push(format!("timestamp={}", timestamp));
        sign_parts.push(format!("window={}", window));

        // 生成最終簽名字符串
        let sign_str = sign_parts.join("&");

        // 使用 ED25519 進行簽名
        let signature = self.signing_key.sign(sign_str.as_bytes());
        let signature_b64 = general_purpose::STANDARD.encode(signature.to_bytes());

        // 返回時間戳、窗口大小和簽名
        Ok((timestamp.to_string(), window.to_string(), signature_b64))
    }

    // 獲取指令類型
    fn get_instruction_type(&self, endpoint: &str, method: &str) -> String {
        match (endpoint, method) {
            ("/api/v1/account", "GET") => "accountQuery".to_string(),
            ("/api/v1/account/limits/order", "GET") => "maxOrderQuantity".to_string(),
            ("/api/v1/order", "POST") => "orderExecute".to_string(),
            ("/api/v1/order", "DELETE") => "orderCancel".to_string(),
            _ => "unknown".to_string(),
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

// Data structures for API requests and responses

#[derive(Serialize)]
pub struct PlaceOrderRequest {
    #[serde(rename = "autoBorrow")]
    pub auto_borrow: Option<bool>,
    #[serde(rename = "autoBorrowRepay")]
    pub auto_borrow_repay: Option<bool>,
    pub symbol: String,
    #[serde(rename = "side")]
    pub side: String,
    #[serde(rename = "orderType")]
    pub order_type: String,
    #[serde(rename = "timeInForce")]
    pub time_in_force: String,
    pub price: Option<String>,
    pub quantity: String,
    #[serde(rename = "clientId")]
    pub client_id: Option<u32>,
}

#[derive(Serialize)]
pub struct CancelOrderRequest {
    pub symbol: String,
    #[serde(rename = "orderId")]
    pub order_id: String,
}

#[derive(Deserialize)]
pub struct Account {
    #[serde(rename = "autoBorrowSettlements")]
    pub auto_borrow_settlements: bool,
    #[serde(rename = "autoLend")]
    pub auto_lend: bool,
    #[serde(rename = "autoRealizePnl")]
    pub auto_realize_pnl: bool,
    #[serde(rename = "autoRepayBorrows")]
    pub auto_repay_borrows: bool,
    #[serde(rename = "borrowLimit")]
    pub borrow_limit: String,
    #[serde(rename = "futuresMakerFee")]
    pub futures_maker_fee: String,
    #[serde(rename = "futuresTakerFee")]
    pub futures_taker_fee: String,
    #[serde(rename = "leverageLimit")]
    pub leverage_limit: String,
    #[serde(rename = "limitOrders")]
    pub limit_orders: u32,
    pub liquidating: bool,
    #[serde(rename = "positionLimit")]
    pub position_limit: String,
    #[serde(rename = "spotMakerFee")]
    pub spot_maker_fee: String,
    #[serde(rename = "spotTakerFee")]
    pub spot_taker_fee: String,
    #[serde(rename = "triggerOrders")]
    pub trigger_orders: u32,
}

#[derive(Deserialize)]
pub struct MaxOrderQuantity {
    #[serde(rename = "maxOrderQuantity")]
    pub max_order_quantity: String,
}

#[derive(Deserialize, Clone)]
pub struct Order {
    pub id: String,
    #[serde(rename = "clientId")]
    pub client_id: Option<u32>,
    #[serde(rename = "orderType")]
    pub order_type: String,
    pub side: String,
    pub status: String,
    pub symbol: String,
    pub price: Option<String>,
    pub quantity: String,
    #[serde(rename = "executedQuantity")]
    pub executed_quantity: String,
    #[serde(rename = "quoteQuantity")]
    pub quote_quantity: Option<String>,
    #[serde(rename = "executedQuoteQuantity")]
    pub executed_quote_quantity: Option<String>,
    #[serde(rename = "timeInForce")]
    pub time_in_force: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
}
