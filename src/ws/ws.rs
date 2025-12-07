use bitreader::BitReader;
use std::str::from_utf8;

use anyhow::anyhow;
use base64::prelude::*;
use http_body_util::combinators::BoxBody;
use hyper::{
    body::Bytes,
    header::{
        HeaderValue, CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION,
        UPGRADE,
    },
    HeaderMap, Request, Response, StatusCode, Version,
};
use hyper_util::rt::TokioIo;
use sha1::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

use crate::utils::creators::{empty, full};

// TODO: server should keep track of connections, make sure to
// add some kind of map for users connected or smth so we don't handshake the
// same connectionmultiple times. For now implement it so our connection works.

pub async fn connect_ws(
    mut req: Request<hyper::body::Incoming>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let mut res = Response::new(empty());
    match ClientWebsocketUpgradeHeaders::from_headers(req.headers()) {
        Ok(client_upgrade_headers) => {
            println!("Client upgrade headers: {:?}", client_upgrade_headers);
            //TODO: I don't know how to handle websocket versions yet (I should just stick to the
            //newest)
            let serv_res_headers =
                ServerWebsocketUpgradeHeaders::from_client_headers(&client_upgrade_headers);
            *res.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
            res.headers_mut().insert(
                CONNECTION,
                HeaderValue::from_str(&serv_res_headers.connection).unwrap(),
            );
            res.headers_mut().insert(
                UPGRADE,
                HeaderValue::from_str(&serv_res_headers.upgrade).unwrap(),
            );
            res.headers_mut().insert(
                SEC_WEBSOCKET_ACCEPT,
                HeaderValue::from_str(&serv_res_headers.sec_websocket_accept).unwrap(),
            );
        }
        Err(err) => {
            println!("Failed to upgrade request due to {}", err);
            *res.status_mut() = StatusCode::BAD_REQUEST;
        }
    }

    println!("Connected client");
    //TODO: implement frame exchagne on a separate thread
    tokio::task::spawn(async move {
        match hyper::upgrade::on(&mut req).await {
            Ok(upgraded) => {
                if let Err(e) = upgraded_ws_conn(upgraded).await {
                    eprint!("server error: {}", e)
                };
            }
            Err(e) => eprint!("server error: {}", e),
        }
    });

    Ok(res)
}

async fn upgraded_ws_conn(upgraded: hyper::upgrade::Upgraded) -> anyhow::Result<()> {
    println!("Upgraded connection");
    let mut upgraded = TokioIo::new(upgraded);
    // If masking is used, the next 4 bytes contain the masking key (see
    let mut frame_bytes = vec![0; 4];
    upgraded.read_exact(&mut frame_bytes).await?;
    println!("readbytes: {:?}", frame_bytes.as_slice());

    let mut reader = BitReader::new(frame_bytes.as_slice());
    let mut i = 0;
    for _ in 0..32 {
        i = i + 1;
        print!("{}", reader.read_u8(1)?);
        if i % 8 == 0 {
            println!();
        }
    }
    println!();
    let mut frame = Frame::from_bytes(frame_bytes.as_slice())?;
    if frame.mask == 1 {
        let mut bytes = vec![0; 4];
        upgraded.read_exact(&mut bytes).await?;
        frame.set_masking_key(bytes);
    }
    println!("{:?}", frame);
    match frame.opcode {
        0x0 => {
            println!("OPCODE continuation");
        }
        0x1 => {
            println!("OPCODE text");
            let len: usize = frame.payload_length.try_into().unwrap();
            println!("got len={}", len);
            let mut payload = vec![0u8;len];
            println!("payload before: {:?}", payload);
            upgraded.read_exact(&mut payload).await?;
            println!("paylaod after: {:?}", payload);
            let decoded = frame.decode(payload.as_mut_slice());
            println!("decoded_raw: {:?}", decoded);
            let s = String::from_utf8(decoded)?;
            println!("chatter: {}", s);
        }
        0x2 => {
            println!("OPCODE binary data");
        }
        _ => {
            println!("Skipping bytes 3-F");
        }
    }
    Ok(())
}

// First byte:
// bit 0: FIN
// bit 1: RSV1
// bit 2: RSV2
// bit 3: RSV3
// bits 4-7 OPCODE
// Bytes 2-10: payload length (see Decoding Payload Length)
// All subsequent bytes are payload
// async fn read_frame(){
//
// }

pub async fn handle_ws_connection(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let http_version = req.version();
    match http_version {
        Version::HTTP_09 | Version::HTTP_10 => {
            let mut res = Response::new(full("Older HTTP version used, please use HTTP/1.1+"));
            *res.status_mut() = hyper::StatusCode::HTTP_VERSION_NOT_SUPPORTED;
            Ok(res)
        }
        //HTTP/1.1 +
        _ => connect_ws(req).await,
    }
}
/*
 Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==
Sec-WebSocket-Version: 13
 */
#[derive(Debug)]
struct ClientWebsocketUpgradeHeaders {
    //TODO: owned strings for now, think about this when ws conn works
    upgrade: String,
    connection: String,
    sec_websocket_key: String,
    _sec_websocket_version: u8,
}
impl ClientWebsocketUpgradeHeaders {
    /// Create a struct containing headers that are required to make a WS handshake
    /// Error if any of headers is not present.
    /// Required headers are:
    /// - Upgrade
    /// - Connection
    /// - Sec-WebSocket-Key
    /// - Sec-WebSocket-Version
    fn from_headers(headers: &HeaderMap<HeaderValue>) -> Result<Self, anyhow::Error> {
        //Maybe just do headers.get() and if err then return Err
        if !headers.contains_key(UPGRADE) {
            return Err(anyhow!("Missing Upgrade header"));
        }
        if !headers.contains_key(CONNECTION) {
            return Err(anyhow!("Missing Connection header"));
        }
        if !headers.contains_key(SEC_WEBSOCKET_KEY) {
            return Err(anyhow!("Missing Sec-WebSocket-Key header"));
        }
        if !headers.contains_key(SEC_WEBSOCKET_VERSION) {
            return Err(anyhow!("Missing Sec-WebSocket-Version header"));
        }
        //TODO: yikes
        Ok(Self {
            upgrade: headers.get(UPGRADE).unwrap().to_str().unwrap().to_owned(),
            connection: headers
                .get(CONNECTION)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
            sec_websocket_key: headers
                .get(SEC_WEBSOCKET_KEY)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
            _sec_websocket_version: headers
                .get(SEC_WEBSOCKET_VERSION)
                .unwrap()
                .to_str()
                .unwrap()
                .parse()
                .unwrap(),
        })
    }
}
/*
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=
*/
struct ServerWebsocketUpgradeHeaders {
    //TODO: owned strings for now, think about this when ws conn works
    upgrade: String,
    connection: String,
    sec_websocket_accept: String,
}
impl ServerWebsocketUpgradeHeaders {
    /// Returns headers required to be returned by the server
    /// to switch protocol to WebSocket.
    /// Those headers are:
    /// - Upgrade
    /// - Connection
    /// - Sec-WebSocket-Accept
    fn from_client_headers(client_request_headers: &ClientWebsocketUpgradeHeaders) -> Self {
        //TODO: gen the magic_str:
        let magic_string = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
        //add magic to Sec-WebSocket-Key
        let mut sec_websocket_accept = client_request_headers.sec_websocket_key.clone();
        sec_websocket_accept.push_str(magic_string);
        //sha1 it
        let mut hasher = sha1::Sha1::new();
        hasher.update(sec_websocket_accept);
        let hash_result = hasher.finalize();
        //base64 it
        let sec_websocket_accept = BASE64_STANDARD.encode(hash_result);

        Self {
            upgrade: client_request_headers.upgrade.clone(),
            connection: client_request_headers.connection.clone(),
            sec_websocket_accept,
        }
    }
}

#[derive(Debug)]
struct Frame {
    fin: u8,
    rsv1: u8,
    rsv2: u8,
    rsv3: u8,
    opcode: u8,
    mask: u8,
    payload_length: u64,
    masking_key: Vec<u8>,
}

impl Frame {
    fn from_bytes(frame_bytes: &[u8]) -> Result<Self, anyhow::Error> {
        println!("Frame_bytes len: {}", frame_bytes.len());
        let mut bit_reader = BitReader::new(frame_bytes);
        let fin = bit_reader.read_u8(1)?;
        let rsv1 = bit_reader.read_u8(1)?;
        let rsv2 = bit_reader.read_u8(1)?;
        let rsv3 = bit_reader.read_u8(1)?;
        let opcode = bit_reader.read_u8(4)?;
        let mask = bit_reader.read_u8(1)?;
        if mask != 1 {
            return Err(anyhow!("Recevied unmasked frame"));
        }
        let payload_decision = bit_reader.read_u64(7)?;
        println!("decision: {:?}", payload_decision);
        //TODO Basically here after decoding the length we must return the bytes
        //to the caller so they can pull the rest of the data like masking key and payload data
        //because payload data will be right after the masking key
        //and if payload is <=125 then the masking key starts from
        //bit 16 to 48
        let payload_length = match payload_decision {
            0..=125 => payload_decision,
            126 => bit_reader.read_u64(16)?,
            127 => bit_reader.read_u64(64)?,
            _ => return Err(anyhow!("Couldnt parse payload length")),
        };
        let masking_key = Vec::new();
        Ok(Frame {
            fin,
            rsv1,
            rsv2,
            rsv3,
            opcode,
            mask,
            payload_length,
            masking_key,
        })
    }
    fn set_masking_key(&mut self, bytes: Vec<u8>) {
        self.masking_key = bytes;
    }
    fn decode(&self, encoded: &mut [u8]) -> Vec<u8> {
        if self.mask == 1 {
            return encoded
                .iter_mut()
                .enumerate()
                .map(|(idx, val)| *val ^ self.masking_key.get(idx % 4).unwrap())
                .collect();
        } else {
            return Vec::from(encoded);
        }
    }
}
