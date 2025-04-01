FROM rustembedded/cross:x86_64-unknown-linux-gnu-0.2.1

# 安裝相依套件
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    openssl \
    cmake

# 設定交叉編譯環境變數
ENV PKG_CONFIG_ALLOW_CROSS=1 
ENV OPENSSL_DIR=/usr/lib/x86_64-linux-gnu/
ENV OPENSSL_INCLUDE_DIR=/usr/include/openssl
ENV OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu/

# 建立工作目錄
WORKDIR /usr/src/bp_quant_trading

# 複製 Cargo.toml 及 Cargo.lock（如果存在）
COPY Cargo.toml Cargo.lock* ./
# 建立假的 main.rs 以快取相依項
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
# 建置相依項以快取
RUN cargo build --release --target x86_64-unknown-linux-gnu 

# 複製實際的源碼
COPY . .
# 建置實際的專案
RUN cargo build --release --target x86_64-unknown-linux-gnu

# 提取編譯好的二進制檔案到一個 scratch 容器中
FROM scratch AS export-stage
COPY --from=0 /usr/src/bp_quant_trading/target/x86_64-unknown-linux-gnu/release/backpack-hft-bot /
