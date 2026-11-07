use crate::*;
use reqwest::{Client, redirect::Policy};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use url::{Host, Url};
const MAX_RESPONSE: u64 = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) fn endpoint(config: &SummaryConfig) -> Result<Url> {
    let default = match config.provider {
        Provider::Local => "http://127.0.0.1:11434/v1",
        Provider::OpenAi => "https://api.openai.com/v1",
        Provider::Anthropic => "https://api.anthropic.com/v1",
        _ => "",
    };
    let mut url = Url::parse(config.endpoint.as_deref().unwrap_or(default))
        .map_err(|_| SummaryError::Config("invalid provider endpoint"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(SummaryError::Config(
            "endpoint may not contain credentials, query, or fragment",
        ));
    }
    if config.provider == Provider::Local {
        // Numeric loopback only: no DNS rebinding, proxy, mDNS or private-network classification.
        let loopback = match url.host() {
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if !loopback || !matches!(url.scheme(), "http" | "https") {
            return Err(SummaryError::Config(
                "local endpoint must use a numeric loopback address",
            ));
        }
    } else if url.scheme() != "https" || url.host_str().is_none() {
        return Err(SummaryError::Config("cloud endpoints require HTTPS"));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}
fn client(timeout: Duration) -> Result<Client> {
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(timeout)
        .build()
        .map_err(|_| SummaryError::Transport("client"))
}
pub(crate) fn configured(
    config: &SummaryConfig,
    helper: Option<&Path>,
) -> Result<Box<dyn Summarizer>> {
    validate_config(config)?;
    if config.provider == Provider::Apple {
        return Ok(Box::new(AppleProvider {
            path: helper
                .ok_or_else(|| SummaryError::Unavailable("Apple helper is missing".into()))?
                .to_owned(),
        }));
    }
    let secret = crate::secret_get(config)?;
    if config.provider.is_cloud() && secret.is_none() {
        return Err(SummaryError::Config(
            "save a provider credential in Keychain",
        ));
    }
    Ok(Box::new(HttpProvider {
        config: config.clone(),
        base: endpoint(config)?,
        client: client(TIMEOUT)?,
        secret,
    }))
}
struct HttpProvider {
    config: SummaryConfig,
    base: Url,
    client: Client,
    secret: Option<String>,
}
impl Summarizer for HttpProvider {
    fn generate(
        &self,
        request: &GenerationRequest,
        cancel: &Arc<AtomicBool>,
    ) -> Result<SummaryContent> {
        check_cancel(cancel)?;
        let anthropic = self.config.provider == Provider::Anthropic;
        let path = if anthropic {
            "messages"
        } else {
            "chat/completions"
        };
        let model = self.config.model.as_deref().unwrap_or_default();
        let mut body = if anthropic {
            json!({"model":model,"max_tokens":request.max_output_tokens,"system":request.instructions,"messages":[{"role":"user","content":request.input}]})
        } else {
            json!({"model":model,"messages":[{"role":"system","content":request.instructions},{"role":"user","content":request.input}],"max_completion_tokens":request.max_output_tokens,"response_format":{"type":"json_object"},"stream":false,"store":false})
        };
        if !anthropic && self.config.provider != Provider::OpenAi {
            body.as_object_mut()
                .unwrap()
                .remove("max_completion_tokens");
            body["max_tokens"] = request.max_output_tokens.into();
            body.as_object_mut().unwrap().remove("store");
        }
        let mut builder = self
            .client
            .post(
                self.base
                    .join(path)
                    .map_err(|_| SummaryError::Config("invalid endpoint path"))?,
            )
            .json(&body);
        if let Some(secret) = &self.secret {
            builder = if anthropic {
                builder
                    .header("x-api-key", secret)
                    .header("anthropic-version", "2023-06-01")
            } else {
                builder.bearer_auth(secret)
            };
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SummaryError::Transport("runtime"))?;
        runtime.block_on(async {
            let operation=async {
                let mut response=builder.send().await.map_err(|_|SummaryError::Transport("network"))?;
                if !response.status().is_success(){return Err(SummaryError::Transport(match response.status().as_u16(){401|403=>"authentication",429=>"rate limited",300..=399=>"redirect refused",_=>"HTTP status"}));}
                let mut bytes=Vec::new();
                while let Some(chunk)=response.chunk().await.map_err(|_|SummaryError::Transport("response"))? {if bytes.len()+chunk.len()>MAX_RESPONSE as usize{return Err(SummaryError::InvalidOutput);}bytes.extend_from_slice(&chunk);}
                let value:Value=serde_json::from_slice(&bytes).map_err(|_|SummaryError::InvalidOutput)?;
                let text=if anthropic {
                    if value.get("stop_reason").and_then(Value::as_str)==Some("refusal"){return Err(SummaryError::Refused);}
                    if value.get("stop_reason").and_then(Value::as_str)==Some("max_tokens"){return Err(SummaryError::InvalidOutput);}
                    value.get("content").and_then(Value::as_array).and_then(|a|a.iter().find(|v|v.get("type").and_then(Value::as_str)==Some("text"))).and_then(|v|v.get("text")).and_then(Value::as_str)
                }else{
                    if value.pointer("/choices/0/message/refusal").and_then(Value::as_str).is_some(){return Err(SummaryError::Refused);}
                    if value.pointer("/choices/0/finish_reason").and_then(Value::as_str)==Some("length"){return Err(SummaryError::InvalidOutput);}
                    value.pointer("/choices/0/message/content").and_then(Value::as_str)
                }.ok_or(SummaryError::InvalidOutput)?;
                serde_json::from_str(text).map_err(|_|SummaryError::InvalidOutput)
            };
            tokio::pin!(operation);
            loop {tokio::select! {result=&mut operation=>return result,_=tokio::time::sleep(Duration::from_millis(40))=>check_cancel(cancel)?,}}
        })
    }
}
struct AppleProvider {
    path: PathBuf,
}
impl Summarizer for AppleProvider {
    fn generate(
        &self,
        request: &GenerationRequest,
        cancel: &Arc<AtomicBool>,
    ) -> Result<SummaryContent> {
        let bytes = run_helper(
            &self.path,
            &[],
            Some(serde_json::to_vec(request).map_err(|_| SummaryError::InvalidOutput)?),
            cancel,
            TIMEOUT,
        )?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| SummaryError::InvalidOutput)?;
        match value.get("error").and_then(Value::as_str) {
            Some("refused") => Err(SummaryError::Refused),
            Some("context_budget") => Err(SummaryError::ContextBudget),
            Some(_) => Err(SummaryError::Unavailable(
                "Apple model could not generate notes".into(),
            )),
            None => serde_json::from_value(value).map_err(|_| SummaryError::InvalidOutput),
        }
    }
}
fn run_helper(
    path: &Path,
    args: &[&str],
    input: Option<Vec<u8>>,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Vec<u8>> {
    check_cancel(cancel)?;
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| SummaryError::Unavailable("Apple helper could not start".into()))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or(SummaryError::Transport("helper input"))?;
    if let Some(input) = input
        && stdin.write_all(&input).is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(SummaryError::Transport("helper input"));
    }
    drop(stdin);
    let stdout = child
        .stdout
        .take()
        .ok_or(SummaryError::Transport("helper output"))?;
    let reader = thread::spawn(move || {
        let mut out = Vec::new();
        let result = stdout.take(MAX_RESPONSE + 1).read_to_end(&mut out);
        (result, out)
    });
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return if cancel.load(Ordering::Relaxed) {
                Err(SummaryError::Cancelled)
            } else {
                Err(SummaryError::Transport("helper timeout"))
            };
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let (read, out) = reader
                    .join()
                    .map_err(|_| SummaryError::Transport("helper reader"))?;
                if read.is_err() || out.len() > MAX_RESPONSE as usize {
                    return Err(SummaryError::InvalidOutput);
                }
                if !status.success() {
                    return Err(SummaryError::Unavailable(
                        "Apple helper exited unsuccessfully".into(),
                    ));
                }
                return Ok(out);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(SummaryError::Transport("helper process"));
            }
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppleAvailability {
    pub available: bool,
    pub reason: Option<String>,
    pub context_tokens: u32,
}
pub fn apple_availability(helper: &Path) -> Result<AppleAvailability> {
    let out = run_helper(
        helper,
        &["--availability"],
        None,
        &Arc::new(AtomicBool::new(false)),
        Duration::from_secs(8),
    )?;
    serde_json::from_slice(&out).map_err(|_| SummaryError::InvalidOutput)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelAvailability {
    pub available: bool,
    pub models: Vec<String>,
    pub reason: Option<String>,
}
/// Only read-only numeric-loopback probes; never downloads a model or contacts cloud.
pub fn discover_local_models(endpoint_text: &str) -> Result<ModelAvailability> {
    let config = SummaryConfig {
        provider: Provider::Local,
        endpoint: Some(endpoint_text.into()),
        ..Default::default()
    };
    let base = endpoint(&config)?;
    let client = client(Duration::from_secs(3))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| SummaryError::Transport("runtime"))?;
    runtime.block_on(async {
        let result = async {
            let mut response = client
                .get(
                    base.join("models")
                        .map_err(|_| SummaryError::Config("invalid endpoint"))?,
                )
                .send()
                .await
                .map_err(|_| SummaryError::Transport("unavailable"))?;
            if !response.status().is_success() {
                return Err(SummaryError::Transport("HTTP status"));
            }
            let mut out = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| SummaryError::Transport("response"))?
            {
                if out.len() + chunk.len() > MAX_RESPONSE as usize {
                    return Err(SummaryError::InvalidOutput);
                }
                out.extend_from_slice(&chunk);
            }
            let value: Value =
                serde_json::from_slice(&out).map_err(|_| SummaryError::InvalidOutput)?;
            let mut models = value
                .get("data")
                .and_then(Value::as_array)
                .ok_or(SummaryError::InvalidOutput)?
                .iter()
                .filter_map(|v| v.get("id").and_then(Value::as_str))
                .filter(|s| s.len() <= 200)
                .take(100)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            models.sort();
            models.dedup();
            Ok(ModelAvailability {
                available: true,
                models,
                reason: None,
            })
        }
        .await;
        match result {
            Ok(value) => Ok(value),
            Err(_) => Ok(ModelAvailability {
                available: false,
                models: Vec::new(),
                reason: Some(
                    "No accessible OpenAI-compatible model server at this loopback endpoint."
                        .into(),
                ),
            }),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::{TcpListener, TcpStream},
        sync::mpsc,
    };
    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some((key, value)) = line.split_once(':')
                && key.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().unwrap();
            }
            request.push_str(&line);
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        request.push_str(&String::from_utf8(body).unwrap());
        request
    }
    fn server(reply: String) -> (Url, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/v1/", listener.local_addr().unwrap())).unwrap();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            tx.send(read_request(&mut stream)).unwrap();
            let _ = stream.write_all(reply.as_bytes());
        });
        (url, rx, handle)
    }
    fn json_response(value: Value) -> String {
        let body = value.to_string();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }
    fn request() -> GenerationRequest {
        GenerationRequest {
            instructions: "Synthetic JSON summary".into(),
            input: "Synthetic data".into(),
            max_output_tokens: 600,
            context_tokens: 4096,
            retry: false,
            template: "general".into(),
        }
    }
    fn provider(base: Url, kind: Provider) -> HttpProvider {
        HttpProvider {
            base,
            config: SummaryConfig {
                provider: kind,
                model: Some("synthetic-model".into()),
                ..Default::default()
            },
            client: client(Duration::from_secs(3)).unwrap(),
            secret: Some("synthetic-credential-only".into()),
        }
    }
    #[test]
    fn local_http_uses_compatible_contract_and_parses_structured_notes() {
        let notes = SummaryContent {
            key_points: vec![CitedItem {
                text: "Synthetic".into(),
                source_ids: vec!["1".into()],
            }],
            ..Default::default()
        };
        let (base, received, server) = server(json_response(
            json!({"choices":[{"message":{"content":serde_json::to_string(&notes).unwrap()},"finish_reason":"stop"}]}),
        ));
        let result = provider(base, Provider::Local)
            .generate(&request(), &Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert_eq!(result, notes);
        let sent = received.recv().unwrap();
        assert!(sent.starts_with("POST /v1/chat/completions"));
        assert!(sent.contains("Bearer synthetic-credential-only"));
        assert!(sent.contains("\"max_tokens\":600"));
        assert!(!sent.contains("max_completion_tokens"));
        server.join().unwrap();
    }
    #[test]
    fn anthropic_contract_has_version_header_and_text_blocks() {
        let (base, received, server) = server(json_response(
            json!({"content":[{"type":"text","text":serde_json::to_string(&SummaryContent::default()).unwrap()}],"stop_reason":"end_turn"}),
        ));
        provider(base, Provider::Anthropic)
            .generate(&request(), &Arc::new(AtomicBool::new(false)))
            .unwrap();
        let sent = received.recv().unwrap();
        assert!(sent.starts_with("POST /v1/messages"));
        assert!(sent.contains("anthropic-version: 2023-06-01"));
        assert!(sent.contains("x-api-key: synthetic-credential-only"));
        assert!(!sent.contains("Bearer"));
        server.join().unwrap();
    }
    #[test]
    fn redirects_do_not_send_credentials_to_another_origin() {
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let (base, received, server) = server(format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{}/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            destination.local_addr().unwrap()
        ));
        let result =
            provider(base, Provider::Local).generate(&request(), &Arc::new(AtomicBool::new(false)));
        assert!(matches!(
            result,
            Err(SummaryError::Transport("redirect refused"))
        ));
        received.recv().unwrap();
        server.join().unwrap();
        assert!(matches!(destination.accept(),Err(e)if e.kind()==std::io::ErrorKind::WouldBlock));
    }
    #[test]
    fn malformed_output_and_refusal_are_separate_errors() {
        for (reply, refused) in [
            (json!({"choices":[{"message":{"content":"{}"}}]}), false),
            (
                json!({"choices":[{"message":{"refusal":"synthetic refusal"}}]}),
                true,
            ),
        ] {
            let (base, received, server) = server(json_response(reply));
            let result = provider(base, Provider::Local)
                .generate(&request(), &Arc::new(AtomicBool::new(false)));
            if refused {
                assert!(matches!(result, Err(SummaryError::Refused)));
            } else {
                assert!(matches!(result, Err(SummaryError::InvalidOutput)));
            }
            received.recv().unwrap();
            server.join().unwrap();
        }
    }
    #[test]
    fn cancelling_an_inflight_http_request_drops_it_promptly() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/v1/", listener.local_addr().unwrap())).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let trigger = flag.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            trigger.store(true, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(300));
        });
        let start = Instant::now();
        let result = provider(base, Provider::Local).generate(&request(), &flag);
        assert!(matches!(result, Err(SummaryError::Cancelled)));
        assert!(start.elapsed() < Duration::from_millis(250));
        server.join().unwrap();
    }
}
