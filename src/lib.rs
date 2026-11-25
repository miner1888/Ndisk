//! NDisk(二次重建版):多用户网盘服务端库。
//!
//! 分层与依赖方向(单向,详见 docs/ARCHITECTURE.md):
//! `web`(HTTP 编排)→ `domain`(纯业务逻辑)→ `db`(SQLite 持久层);
//! `engine`(离线下载引擎)→ `domain` + `db`,由 `app::App` 组装。

#![allow(clippy::result_large_err)]

pub mod app;
pub mod cli;
pub mod db;
pub mod domain;
pub mod engine;
pub mod error;
pub mod tls;
pub mod web;
