//! 配置读写。与原桌面版逻辑一致，只是把 `app.path().app_data_dir()`
//! 换成显式传入的 `config_path`，从而不再依赖 Tauri。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::DownloadFormat;

pub const DEFAULT_API_DOMAIN: &str = "www.wn07.ru";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// 认证 Token（Web 版新增）
    #[serde(default)]
    pub token: String,
    pub cookie: String,
    pub download_dir: PathBuf,
    pub export_dir: PathBuf,
    pub enable_file_logger: bool,
    pub download_format: DownloadFormat,
    pub proxy_mode: ProxyMode,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub comic_concurrency: usize,
    pub comic_download_interval_sec: u64,
    pub img_concurrency: usize,
    pub img_download_interval_sec: u64,
    pub download_shelf_interval_ms: u64,
    pub batch_download_interval_ms: u64,
    pub use_original_filename: bool,
    pub api_domain_mode: ApiDomainMode,
    pub custom_api_domain: String,
}

impl Config {
    /// 从指定路径加载配置。文件不存在则用默认值创建。
    pub fn load(config_path: &Path) -> anyhow::Result<Self> {
        let data_dir = config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let config = if config_path.exists() {
            let config_string = std::fs::read_to_string(config_path)?;
            match serde_json::from_str(&config_string) {
                Ok(config) => config,
                Err(_) => Config::merge_config(&config_string, &data_dir),
            }
        } else {
            Config::default_at(&data_dir)
        };
        config.save(config_path)?;
        Ok(config)
    }

    /// 写回配置文件。
    pub fn save(&self, config_path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let config_string = serde_json::to_string_pretty(self)?;
        std::fs::write(config_path, config_string)?;
        Ok(())
    }

    pub fn get_api_domain(&self) -> String {
        if self.api_domain_mode == ApiDomainMode::Custom {
            self.custom_api_domain.clone()
        } else {
            DEFAULT_API_DOMAIN.to_string()
        }
    }

    fn merge_config(config_string: &str, data_dir: &Path) -> Config {
        let Ok(mut json_value) = serde_json::from_str::<serde_json::Value>(config_string) else {
            return Config::default_at(data_dir);
        };
        let serde_json::Value::Object(ref mut map) = json_value else {
            return Config::default_at(data_dir);
        };
        let Ok(default_config_value) = serde_json::to_value(Config::default_at(data_dir)) else {
            return Config::default_at(data_dir);
        };
        let serde_json::Value::Object(default_map) = default_config_value else {
            return Config::default_at(data_dir);
        };
        for (key, value) in default_map {
            map.entry(key).or_insert(value);
        }
        let Ok(config) = serde_json::from_value(json_value) else {
            return Config::default_at(data_dir);
        };
        config
    }

    /// 默认配置。`data_dir` 是数据根目录（容器里通常是 `/data`）。
    pub fn default_at(data_dir: &Path) -> Config {
        Config {
            token: String::new(),
            cookie: String::new(),
            download_dir: data_dir.join("漫画下载"),
            export_dir: data_dir.join("漫画导出"),
            enable_file_logger: true,
            download_format: DownloadFormat::Jpeg,
            proxy_mode: ProxyMode::System,
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 7890,
            comic_concurrency: 2,
            comic_download_interval_sec: 0,
            img_concurrency: 10,
            img_download_interval_sec: 1,
            download_shelf_interval_ms: 100,
            batch_download_interval_ms: 100,
            use_original_filename: false,
            api_domain_mode: ApiDomainMode::Default,
            custom_api_domain: DEFAULT_API_DOMAIN.to_string(),
        }
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ProxyMode {
    #[default]
    System,
    NoProxy,
    Custom,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ApiDomainMode {
    #[default]
    Default,
    Custom,
}
