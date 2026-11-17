# Zephyr port of the latency measurement (same method and output protocol as
# the Rust firmware). Build with tools/zephyr.sh.
cmake_minimum_required(VERSION 3.20.0)
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(zephyr_latency)

target_sources(app PRIVATE src/main.c)
