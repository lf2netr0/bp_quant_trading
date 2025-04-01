#!/bin/bash
# set -e

# 構建 Docker 映像
docker build -t bp_quant_trading_builder .

# 創建臨時容器來保存編譯結果
CONTAINER_ID=$(docker create bp_quant_trading_builder)

# 從容器中提取編譯好的二進制檔案
mkdir -p target/release
docker cp $CONTAINER_ID:/backpack-hft-bot ./target/release/bp_quant_trading

# 刪除臨時容器
docker rm $CONTAINER_ID

echo "編譯完成，二進制檔案位於 ./target/release/bp_quant_trading"
chmod +x ./target/release/bp_quant_trading
