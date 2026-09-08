//! 认证注入脚本模板与运行时配置组装。

const AUTH_JS_TEMPLATE: &str = include_str!("../../assets/auth/auth.js");
pub const CFG_TOKEN: &str = "__YL_CFG__";

/// 把运行时配置 JSON 填进注入模板，返回可直接 eval 的完整脚本。
pub fn build_auth_js(cfg: &serde_json::Value) -> String {
    AUTH_JS_TEMPLATE.replace(CFG_TOKEN, &cfg.to_string())
}
