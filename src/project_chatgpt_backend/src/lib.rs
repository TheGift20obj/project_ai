use ic_cdk::api::management_canister::http_request;
use ic_cdk::api::management_canister::http_request::HttpMethod;
use ic_cdk::api::management_canister::http_request::HttpResponse;
use ic_cdk::api::management_canister::http_request::CanisterHttpRequestArgument;
use ic_cdk::api::management_canister::http_request::HttpHeader;
use candid::{Principal, CandidType, Nat};
use std::collections::HashMap;
use ic_cdk_macros::{update, query};
use serde_json::json;
use serde::{Deserialize, Serialize};
use ic_cdk::api::time;
use std::env;

const PROMPT_LIMIT: u32 = 50;
const BLOCK_TIME_NANOS: u64 = 12 * 60 * 60 * 1_000_000_000;

#[derive(Clone, CandidType, Deserialize, Serialize)]
struct ChatMeta {
    id: ChatId,
    name: String,
}

#[derive(Clone, CandidType, Deserialize)]
struct ChatMessage {
    question: String,
    answer: String,
}

#[derive(Clone, CandidType, Deserialize)]
struct ChatInfo {
    name: String,
    messages: Vec<ChatMessage>,
}

type ChatId = String;

thread_local! {
    static USER_CHATS: std::cell::RefCell<HashMap<Principal, HashMap<ChatId, ChatInfo>>> = std::cell::RefCell::new(HashMap::new());
    static USER_NAMES: std::cell::RefCell<HashMap<Principal, String>> = std::cell::RefCell::new(HashMap::new());
    static USER_PROMPTS: std::cell::RefCell<HashMap<Principal, (u32, Option<u64>)>> = std::cell::RefCell::new(HashMap::new());
}
#[derive(Serialize, Deserialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<Message>,
}

#[derive(Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

const CYCLES_FOR_HTTP_REQUEST: u128 = 11_000_000_000;

const OPENAI_API_KEY: &str = env!("OPENAI_API_KEY");

fn get_openai_api_key() -> String {
    let key = OPENAI_API_KEY.clone().to_string();
    key
}

pub struct HttpRequest {
    pub url: String,
    pub method: HttpMethod,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

#[update]
async fn chat(prompt: String) -> String {
    let openai_api_key = get_openai_api_key();
    let body = json!({
        "model": "gpt-4o-mini",
        "messages": [{
            "role": "user",
            "content": prompt,
        }]
    });
    
    let body_str = body.to_string();

    let request = CanisterHttpRequestArgument {
        url: "https://api.openai.com/v1/chat/completions".to_string(),
        method: HttpMethod::POST,
        headers: vec![
            HttpHeader {
                name: "Authorization".to_string(),
                value: format!("Bearer {}", openai_api_key),
            },
            HttpHeader {
                name: "Content-Type".to_string(),
                value: "application/json".to_string(),
            },
        ],
        body: Some(body_str.into_bytes()),
        max_response_bytes: Some(1048576), // 1MB limit na odpowiedź
        transform: None,
    };

    match http_request::http_request(request, CYCLES_FOR_HTTP_REQUEST).await { //ic_cdk::api::canister_balance128();
        Ok((HttpResponse { status, body, .. },)) if status == Nat::from(200u16) => {
            //return "Test".to_string();
            let resp_str = String::from_utf8(body).unwrap_or_default();
            // Parsujemy JSON OpenAI response aby zwrócić treść odpowiedzi:
            #[derive(Deserialize)]
            struct Choice {
                message: Message,
            }
            #[derive(Deserialize)]
            struct OpenAIResponse {
                choices: Vec<Choice>,
            }

            match serde_json::from_str::<OpenAIResponse>(&resp_str) {
                Ok(resp) => {
                    if let Some(choice) = resp.choices.first() {
                        choice.message.content.clone()
                    } else {
                        "No choices in response".to_string()
                    }
                }
                Err(e) => format!("JSON parse error: {}", e),
            }
        }
        Ok((HttpResponse { status, body, .. },)) => {
            let error_body = String::from_utf8_lossy(&body);
            format!("OpenAI error status: {}, body: {}", status, error_body)
        }
        Err(e) => format!("HTTP request error: {:?}", e),
    }
}

#[update]
fn create_new_chat(user: Principal, chat_id: ChatId, name: String) {
    USER_CHATS.with(|user_chats| {
        let mut user_chats = user_chats.borrow_mut();
        let chats = user_chats.entry(user).or_insert_with(HashMap::new);
        chats.entry(chat_id).or_insert(ChatInfo {
            name,
            messages: Vec::new(),
        });
    });
}

#[update]
fn add_chat_message(user: Principal, chat_id: ChatId, question: String, answer: String) {
    USER_CHATS.with(|user_chats| {
        let mut user_chats = user_chats.borrow_mut();
        if let Some(chats) = user_chats.get_mut(&user) {
            if let Some(chat_info) = chats.get_mut(&chat_id) {
                chat_info.messages.push(ChatMessage { question, answer });
            }
        }
    });
}

#[query]
fn get_chat_history(user: Principal, chat_id: ChatId) -> ChatInfo {
    USER_CHATS.with(|user_chats| {
        user_chats.borrow()
            .get(&user)
            .and_then(|chats| chats.get(&chat_id))
            .cloned()
    }).expect("REASON")
}

#[update]
fn delete_chat(user: Principal, chat_id: ChatId) -> bool {
    USER_CHATS.with(|user_chats| {
        let mut user_chats = user_chats.borrow_mut();
        if let Some(chats) = user_chats.get_mut(&user) {
            return chats.remove(&chat_id).is_some();
        }
        false
    })
}

#[update]
fn rename_chat(user: Principal, chat_id: ChatId, new_name: String) -> bool {
    USER_CHATS.with(|user_chats| {
        let mut user_chats = user_chats.borrow_mut();
        if let Some(chats) = user_chats.get_mut(&user) {
            if let Some(chat_info) = chats.get_mut(&chat_id) {
                chat_info.name = new_name;
                return true;
            }
        }
        false
    })
}

#[query]
fn list_chats(user: Principal) -> Vec<ChatMeta> {
    USER_CHATS.with(|user_chats| {
        user_chats.borrow()
            .get(&user)
            .map(|chats| {
                chats.iter()
                    .map(|(chat_id, info)| ChatMeta {
                        id: chat_id.clone(),
                        name: info.name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

#[update]
fn set_user_name(user: Principal, name: String) {
    USER_NAMES.with(|names| {
        names.borrow_mut().insert(user, name);
    });
}

#[query]
fn get_user_name(user: Principal) -> String {
    USER_NAMES.with(|names| {
        names
            .borrow()
            .get(&user)
            .cloned()
            .unwrap_or_else(|| "user".to_string())
    })
}

#[update]
pub fn try_increment_user_prompt(user: Principal) -> bool {
    let now = time();

    USER_PROMPTS.with(|map| {
        let mut map = map.borrow_mut();
        let entry = map.entry(user).or_insert((0, None));

        let (count, blocked_since) = entry;

        if let Some(block_time) = blocked_since {
            if now - *block_time >= BLOCK_TIME_NANOS {
                *count = 1;
                *blocked_since = None;
                return true;
            } else {
                return false;
            }
        } else {
            *count += 1;

            if *count >= PROMPT_LIMIT {
                *blocked_since = Some(now);
            }

            return true;
        }
    })
}