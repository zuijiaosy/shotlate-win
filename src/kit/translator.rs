//! OpenAI-compatible chat translation (DeepSeek by default) and an in-memory cache (Translator.swift).
//! All paragraphs of one screenshot go in one request so the model sees the whole context.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct TranslationConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub target_language: String,
    pub timeout: Duration,
}

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-flash";
pub const DEFAULT_TARGET_LANGUAGE: &str = "简体中文";
pub const LANGUAGES: [&str; 5] = ["简体中文", "繁體中文", "English", "日本語", "한국어"];

#[derive(Clone, Debug, PartialEq)]
pub enum TranslationError {
    MissingApiKey,
    InvalidBaseUrl(String),
    Http { status: u16, message: String },
    Network(String),
    BadResponse(String),
}

impl std::fmt::Display for TranslationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranslationError::MissingApiKey => write!(f, "还没有填写 API Key。请在托盘图标 → 设置 → 翻译 中填写。"),
            TranslationError::InvalidBaseUrl(url) => write!(f, "Base URL 无效：{url}"),
            TranslationError::Http { status, message } => match status {
                401 => write!(f, "API Key 无效（401）。请在设置中检查 Key。"),
                402 => write!(f, "账户余额不足（402）。请到 DeepSeek 开放平台充值。"),
                429 => write!(f, "请求太频繁（429），请稍后再试。"),
                _ => write!(f, "翻译服务返回错误 {status}：{message}"),
            },
            TranslationError::Network(detail) => write!(f, "连接翻译服务失败：{detail}"),
            TranslationError::BadResponse(detail) => write!(f, "翻译结果无法解析：{detail}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: usize,
    pub text: String,
}

fn system_prompt(target_language: &str) -> String {
    format!(
        "你是软件界面与文档翻译器。把输入中每一项的 text 翻译成{target_language}，译文简洁自然，符合界面用语习惯。\n\
         代码、文件路径、网址、数字、快捷键和产品名保持原样。\n\
         只输出 JSON，格式为 {{\"items\":[{{\"id\":0,\"t\":\"译文\"}}]}}，id 与输入一一对应，不要遗漏或增加条目。"
    )
}

/// The endpoint URL and JSON body of a translation request.
pub fn make_request(items: &[Item], config: &TranslationConfig) -> Result<(String, Value), TranslationError> {
    if config.api_key.trim().is_empty() {
        return Err(TranslationError::MissingApiKey);
    }
    let base = config.base_url.trim().trim_end_matches('/');
    let valid = (base.starts_with("http://") || base.starts_with("https://")) && base.split("://").nth(1).is_some_and(|h| !h.is_empty());
    if !valid {
        return Err(TranslationError::InvalidBaseUrl(config.base_url.clone()));
    }
    // Sorted keys, like the macOS version, so identical input makes identical requests.
    let input: Vec<Value> = items.iter().map(|i| json!({"id": i.id, "text": i.text})).collect();
    let input_json = serde_json::to_string(&json!({ "items": input })).unwrap_or_default();
    let body = json!({
        "model": config.model,
        "response_format": {"type": "json_object"},
        "temperature": 0.3,
        "stream": false,
        "messages": [
            {"role": "system", "content": system_prompt(&config.target_language)},
            {"role": "user", "content": input_json},
        ],
    });
    Ok((format!("{base}/chat/completions"), body))
}

/// Parses a chat completion response into `id → translation`. Items the model dropped are absent.
pub fn parse_response(data: &[u8]) -> Result<HashMap<usize, String>, TranslationError> {
    let root: Value = serde_json::from_slice(data).map_err(|_| TranslationError::BadResponse("缺少 choices[0].message.content".into()))?;
    let content = root["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| TranslationError::BadResponse("缺少 choices[0].message.content".into()))?;
    parse_content(content)
}

pub fn parse_content(content: &str) -> Result<HashMap<usize, String>, TranslationError> {
    let mut text = content.trim().to_string();
    // Some models wrap JSON in a Markdown code fence despite json_object mode.
    if text.starts_with("```") {
        text = text.lines().skip(1).collect::<Vec<_>>().join("\n");
        if let Some(i) = text.rfind("```") {
            text.truncate(i);
        }
    }
    let object: Value = serde_json::from_str(&text).map_err(|_| TranslationError::BadResponse("返回内容不是预期的 JSON".into()))?;
    let items = object["items"].as_array().ok_or_else(|| TranslationError::BadResponse("返回内容不是预期的 JSON".into()))?;
    let mut result = HashMap::new();
    for item in items {
        let id = match &item["id"] {
            Value::Number(n) => n.as_u64().map(|v| v as usize),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        };
        let translation = item.get("t").or_else(|| item.get("zh")).or_else(|| item.get("translation")).and_then(Value::as_str);
        if let (Some(id), Some(t)) = (id, translation) {
            if !t.is_empty() {
                result.insert(id, t.to_string());
            }
        }
    }
    Ok(result)
}

fn error_message(data: &[u8]) -> String {
    serde_json::from_slice::<Value>(data)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(&data[..data.len().min(200)]).into_owned())
}

/// Sends one request and returns the translations (blocking; run it off the UI thread).
pub fn translate(items: &[Item], config: &TranslationConfig) -> Result<HashMap<usize, String>, TranslationError> {
    if items.is_empty() {
        return Ok(HashMap::new());
    }
    let (url, body) = make_request(items, config)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(config.timeout))
        .http_status_as_error(false)
        .tls_config(super::http::tls())
        .build()
        .into();
    let response = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {}", config.api_key.trim()))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| TranslationError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    let data = response.into_body().read_to_vec().map_err(|e| TranslationError::Network(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(TranslationError::Http { status, message: error_message(&data) });
    }
    parse_response(&data)
}

/// In-memory cache so toggling or re-translating the same text costs nothing.
#[derive(Default)]
pub struct TranslationCache {
    storage: Mutex<HashMap<String, String>>,
}

impl TranslationCache {
    fn key(text: &str, config: &TranslationConfig) -> String {
        format!("{}\u{1F}{}\u{1F}{}", config.model, config.target_language, text)
    }

    /// Translates `items`, sending only the texts that are not cached yet.
    pub fn translate(
        &self,
        items: &[Item],
        config: &TranslationConfig,
        send: impl FnOnce(&[Item]) -> Result<HashMap<usize, String>, TranslationError>,
    ) -> Result<HashMap<usize, String>, TranslationError> {
        let mut result = HashMap::new();
        let mut missing = Vec::new();
        {
            let cache = self.storage.lock().unwrap_or_else(|e| e.into_inner());
            for item in items {
                match cache.get(&Self::key(&item.text, config)) {
                    Some(t) => {
                        result.insert(item.id, t.clone());
                    }
                    None => missing.push(item.clone()),
                }
            }
        }
        if !missing.is_empty() {
            let fresh = send(&missing)?;
            let mut cache = self.storage.lock().unwrap_or_else(|e| e.into_inner());
            for item in &missing {
                if let Some(t) = fresh.get(&item.id) {
                    result.insert(item.id, t.clone());
                    cache.insert(Self::key(&item.text, config), t.clone());
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(key: &str) -> TranslationConfig {
        TranslationConfig {
            base_url: "https://api.deepseek.com/".into(),
            model: DEFAULT_MODEL.into(),
            api_key: key.into(),
            target_language: DEFAULT_TARGET_LANGUAGE.into(),
            timeout: Duration::from_secs(20),
        }
    }

    #[test]
    fn request_shape() {
        let (url, body) = make_request(&[Item { id: 0, text: "Hello".into() }], &config("sk-x")).unwrap();
        assert_eq!(url, "https://api.deepseek.com/chat/completions");
        assert_eq!(body["model"], "deepseek-flash");
        assert_eq!(body["response_format"]["type"], "json_object");
        assert!(body["messages"][1]["content"].as_str().unwrap().contains("\"text\":\"Hello\""));
        assert_eq!(make_request(&[], &config(" ")), Err(TranslationError::MissingApiKey));
        let mut bad = config("k");
        bad.base_url = "ftp://x".into();
        assert!(matches!(make_request(&[], &bad), Err(TranslationError::InvalidBaseUrl(_))));
    }

    #[test]
    fn parses_fenced_and_string_ids() {
        let content = "```json\n{\"items\":[{\"id\":\"0\",\"t\":\"你好\"},{\"id\":1,\"zh\":\"世界\"},{\"id\":2,\"t\":\"\"}]}\n```";
        let r = parse_content(content).unwrap();
        assert_eq!(r.get(&0).unwrap(), "你好");
        assert_eq!(r.get(&1).unwrap(), "世界");
        assert!(!r.contains_key(&2));
        let response = serde_json::to_vec(&json!({"choices":[{"message":{"content":"{\"items\":[{\"id\":0,\"t\":\"好\"}]}"}}]})).unwrap();
        assert_eq!(parse_response(&response).unwrap().get(&0).unwrap(), "好");
        assert!(parse_response(b"{}").is_err());
    }

    #[test]
    fn cache_sends_only_missing() {
        let cache = TranslationCache::default();
        let c = config("k");
        let items = vec![Item { id: 0, text: "a".into() }, Item { id: 1, text: "b".into() }];
        let first = cache.translate(&items, &c, |m| Ok(m.iter().map(|i| (i.id, format!("<{}>", i.text))).collect())).unwrap();
        assert_eq!(first.len(), 2);
        let again = vec![Item { id: 5, text: "b".into() }, Item { id: 6, text: "c".into() }];
        let second = cache
            .translate(&again, &c, |m| {
                assert_eq!(m.len(), 1, "only the uncached text goes out");
                Ok(m.iter().map(|i| (i.id, format!("<{}>", i.text))).collect())
            })
            .unwrap();
        assert_eq!(second.get(&5).unwrap(), "<b>");
        assert_eq!(second.get(&6).unwrap(), "<c>");
    }
}
