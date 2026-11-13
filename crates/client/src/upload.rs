//! Asset uploads through the NewFileAgentInventory capability: the request names the asset and
//! the item to make, the grid offers an uploader (and its price), the asset goes to it, the
//! answer names the new item and asset.
//!
//! Port of parts of aurora-net/src/inventory/upload.rs (Aurora Viewer, see NOTICE.md), after
//! LLResourceUploadInfo::generatePostBody and the upload coroutine of
//! indra/newview/llviewerassetupload.cpp (originally LGPL 2.1): the request's checks, the
//! price check, the bounded answers, the error texts without addresses. Without the legacy UDP
//! upload, the mesh quotes and the inventory re-read.

use ferro_llsd::{Llsd, llsd_map};
use std::time::Duration;
use uuid::Uuid;

/// An answer of the capability or of its uploader larger than this is refused (Aurora's).
const MAX_ANSWER: usize = 64 * 1024;
/// Each of the two requests.
const TIMEOUT: Duration = Duration::from_secs(60);

/// What to upload, and the item to make of it.
#[derive(Debug, Clone)]
pub struct AssetUpload {
    /// LLAssetType's name ("sound") and LLInventoryType's ("sound").
    pub asset_type: &'static str,
    pub inventory_type: &'static str,
    pub folder: Uuid,
    pub name: String,
    pub description: String,
    pub next_owner_mask: u32,
    pub group_mask: u32,
    pub everyone_mask: u32,
    /// The price the bot expects to pay, and will pay no more than; `None` when the grid did not
    /// say it at login: 0 is sent, and the grid's price is taken as it comes.
    pub expected_cost: Option<i32>,
    pub data: Vec<u8>,
}

/// The upload done.
#[derive(Debug, Clone, PartialEq)]
pub struct Uploaded {
    pub item: Uuid,
    pub asset: Uuid,
    /// What the grid says it took.
    pub cost: i32,
}

impl AssetUpload {
    /// The capability's request body (generatePostBody), refused as Aurora refuses it: no
    /// folder, no name, a name or a description too long for the inventory, no data.
    pub fn body(&self) -> Result<Llsd, String> {
        if self.folder.is_nil()
            || self.expected_cost.is_some_and(|c| c < 0)
            || self.name.trim().is_empty()
            || self.name.len() > 63
            || self.description.len() > 127
            || self.name.contains('\0')
            || self.description.contains('\0')
            || self.data.is_empty()
            || self.data.len() > 64 * 1024 * 1024
        {
            return Err("an invalid upload".into());
        }
        Ok(llsd_map! {
            "folder_id" => self.folder,
            "asset_type" => self.asset_type,
            "inventory_type" => self.inventory_type,
            "name" => self.name.as_str(),
            "description" => self.description.as_str(),
            "next_owner_mask" => self.next_owner_mask as i32,
            "group_mask" => self.group_mask as i32,
            "everyone_mask" => self.everyone_mask as i32,
            "expected_upload_cost" => self.expected_cost.unwrap_or(0),
        })
    }
}

/// The grid's own words for a refusal (error.message, as LLMeshUploadThread::log_upload_error
/// reads it): no control characters, no addresses or paths, 256 characters at most.
pub fn refusal_text(v: &Llsd) -> String {
    let error = &v["error"];
    let text = [
        error["message"].as_str(),
        error.as_str(),
        v["message"].as_str(),
    ]
    .into_iter()
    .find(|t| !t.trim().is_empty())
    .unwrap_or("");
    text.split_whitespace()
        .map(|word| {
            let token = word.trim_start_matches(['\'', '"', '(', '[']);
            if token.contains(":/") || token.contains(":\\") || token.starts_with('/') {
                "…"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}

/// The LLSD of an answer, read no further than [`MAX_ANSWER`].
async fn answer(mut resp: reqwest::Response, what: &str) -> Result<Llsd, String> {
    if !resp.status().is_success() {
        return Err(format!("{what}: HTTP {}", resp.status().as_u16()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("{what}: {}", e.without_url()))?
    {
        if bytes.len() + chunk.len() > MAX_ANSWER {
            return Err(format!("{what}: an answer too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    ferro_llsd::from_xml(&bytes).map_err(|_| format!("{what}: an unreadable answer"))
}

/// The upload, as the viewer's coroutine: the capability, then its uploader.
pub(crate) async fn upload(
    http: &reqwest::Client,
    cap: &str,
    request: AssetUpload,
) -> Result<Uploaded, String> {
    let body = request.body()?;
    let sent = http
        .post(cap)
        .header("Content-Type", "application/llsd+xml")
        .header("Accept", "application/llsd+xml")
        .body(ferro_llsd::to_xml(&body))
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("NewFileAgentInventory: {}", e.without_url()))?;
    let offer = answer(sent, "NewFileAgentInventory").await?;
    if offer["state"].as_str() != "upload" || offer["uploader"].as_str().is_empty() {
        let why = refusal_text(&offer);
        return Err(if why.is_empty() {
            "the grid offered no upload".into()
        } else {
            format!("the grid refused it: {why}")
        });
    }
    // the price the bot expects is the most it pays (Aurora's PriceChanged)
    if let Llsd::Integer(price) = &offer["upload_price"]
        && let Some(expected) = request.expected_cost
        && (*price < 0 || *price > expected)
    {
        return Err(format!(
            "the grid asks L${price}, more than the L${expected} expected"
        ));
    }
    let sent = http
        .post(offer["uploader"].as_str())
        .header("Content-Type", "application/octet-stream")
        .body(request.data)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("the upload: {}", e.without_url()))?;
    let ack = answer(sent, "the upload").await?;
    let (item, asset) = (
        ack["new_inventory_item"].as_uuid(),
        ack["new_asset"].as_uuid(),
    );
    if ack["state"].as_str() != "complete" || item.is_nil() || asset.is_nil() {
        let why = refusal_text(&ack);
        return Err(if why.is_empty() {
            "the upload was not completed".into()
        } else {
            format!("the upload was not completed: {why}")
        });
    }
    let cost = match &ack["upload_price"] {
        Llsd::Integer(n) if *n >= 0 => *n,
        _ => request.expected_cost.unwrap_or(0),
    };
    Ok(Uploaded { item, asset, cost })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound() -> AssetUpload {
        AssetUpload {
            asset_type: "sound",
            inventory_type: "sound",
            folder: Uuid::from_u128(1),
            name: "Ding".into(),
            description: String::new(),
            next_owner_mask: 0x7fff_ffff,
            group_mask: 0,
            everyone_mask: 0,
            expected_cost: Some(10),
            data: vec![1, 2, 3],
        }
    }

    #[test]
    fn the_request_as_the_viewer_sends_it() {
        let b = sound().body().expect("a body");
        assert_eq!(b["asset_type"].as_str(), "sound");
        assert_eq!(b["inventory_type"].as_str(), "sound");
        assert_eq!(b["folder_id"].as_uuid(), Uuid::from_u128(1));
        assert_eq!(b["expected_upload_cost"].as_i32(), 10);
        assert_eq!(b["next_owner_mask"].as_i32(), 0x7fff_ffff);
        let unknown = AssetUpload {
            expected_cost: None,
            ..sound()
        };
        assert_eq!(
            unknown.body().expect("a body")["expected_upload_cost"].as_i32(),
            0
        );
        for bad in [
            AssetUpload {
                name: " ".into(),
                ..sound()
            },
            AssetUpload {
                name: "n".repeat(64),
                ..sound()
            },
            AssetUpload {
                folder: Uuid::nil(),
                ..sound()
            },
            AssetUpload {
                data: Vec::new(),
                ..sound()
            },
        ] {
            assert!(bad.body().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn refusals_without_addresses() {
        let v = llsd_map! {
            "state" => "error",
            "error" => llsd_map! { "message" => "Cannot reach https://example.test/x now\n" },
        };
        assert_eq!(refusal_text(&v), "Cannot reach … now");
        assert_eq!(refusal_text(&llsd_map! { "state" => "error" }), "");
    }
}
