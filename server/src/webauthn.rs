//! 패스키(WebAuthn Level 2)의 서버 쪽 검증. 지원 알고리즘은 ES256(P-256)만, 증명(attestation)은 "none".
//! 브라우저·OS 패스키(iCloud 키체인, Google 비밀번호 관리자, Windows Hello)는 모두 ES256을 지원한다.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ciborium::Value;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

const CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING: usize = 10_000;
const FLAG_UP: u8 = 0x01;
/// 지문·PIN 등으로 사용자를 확인함. 패스키 로그인은 비밀번호와 2차 인증을 모두 대신하므로 반드시 요구한다.
const FLAG_UV: u8 = 0x04;
const FLAG_AT: u8 = 0x40;

pub struct WebAuthn {
    rp_id: String,
    origin: String,
    /// 발급한 챌린지 → (용도, 등록하는 사용자, 발급 시각). 한 번 쓰면 지운다.
    challenges: Mutex<HashMap<String, Pending>>,
}

/// (용도, 등록하는 사용자, 발급 시각)
type Pending = (Purpose, Option<i64>, Instant);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    Register,
    Login,
}

/// 등록 결과: 저장할 자격 id와 공개키(SEC1 비압축 점).
pub struct NewCredential {
    pub id: String,
    pub public_key: Vec<u8>,
    pub sign_count: u32,
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
}

#[derive(Deserialize)]
pub struct RegistrationResponse {
    pub id: String,
    pub response: AttestationResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestationResponse {
    // 브라우저 필드 이름은 clientDataJSON이다(camelCase 변환으로는 clientDataJson이 되어 맞지 않는다).
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    pub attestation_object: String,
}

#[derive(Deserialize)]
pub struct AuthenticationResponse {
    pub id: String,
    pub response: AssertionResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionResponse {
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
}

impl WebAuthn {
    pub fn new(origin: &str) -> Self {
        let rp_id = origin
            .split("://")
            .nth(1)
            .unwrap_or(origin)
            .split([':', '/'])
            .next()
            .unwrap_or("localhost")
            .to_owned();
        Self {
            rp_id,
            origin: origin.trim_end_matches('/').to_owned(),
            challenges: Mutex::default(),
        }
    }

    fn new_challenge(&self, purpose: Purpose, user: Option<i64>) -> String {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("OS RNG unavailable");
        let challenge = B64.encode(bytes);
        let mut map = self.challenges.lock().unwrap();
        map.retain(|_, (_, _, at)| at.elapsed() < CHALLENGE_TTL);
        // 로그인하지 않은 요청도 하나씩 쌓으므로 상한을 두고, 넘으면 가장 오래된 것을 버린다.
        if map.len() >= MAX_PENDING
            && let Some(oldest) = map.iter().min_by_key(|(_, (_, _, at))| *at).map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        }
        map.insert(challenge.clone(), (purpose, user, Instant::now()));
        challenge
    }

    fn take_challenge(&self, challenge: &str, purpose: Purpose) -> Option<Option<i64>> {
        let mut map = self.challenges.lock().unwrap();
        let (p, user, at) = map.remove(challenge)?;
        (p == purpose && at.elapsed() < CHALLENGE_TTL).then_some(user)
    }

    /// `navigator.credentials.create`에 넘길 옵션(바이트 값은 base64url 문자열, 브라우저에서 바꾼다).
    pub fn registration_options(
        &self,
        user_id: i64,
        email: &str,
        name: &str,
        exclude: &[String],
    ) -> Json {
        let challenge = self.new_challenge(Purpose::Register, Some(user_id));
        json!({
            "challenge": challenge,
            "rp": { "id": self.rp_id, "name": "Tolkmeet" },
            "user": { "id": B64.encode(user_id.to_be_bytes()), "name": email, "displayName": if name.is_empty() { email } else { name } },
            "pubKeyCredParams": [{ "type": "public-key", "alg": -7 }],
            "authenticatorSelection": { "residentKey": "required", "userVerification": "required" },
            "excludeCredentials": exclude.iter().map(|id| json!({ "type": "public-key", "id": id })).collect::<Vec<_>>(),
            "attestation": "none",
            "timeout": 300_000,
        })
    }

    /// `navigator.credentials.get` 옵션. 자격 목록 없이(기기에 저장된 패스키 중에서 고르게).
    pub fn login_options(&self) -> Json {
        let challenge = self.new_challenge(Purpose::Login, None);
        json!({
            "challenge": challenge,
            "rpId": self.rp_id,
            "userVerification": "required",
            "timeout": 300_000,
        })
    }

    fn check_client_data(
        &self,
        raw_b64: &str,
        kind: &str,
        purpose: Purpose,
    ) -> Result<(Vec<u8>, Option<i64>), &'static str> {
        let raw = B64.decode(raw_b64).map_err(|_| "client data")?;
        let cd: ClientData = serde_json::from_slice(&raw).map_err(|_| "client data")?;
        if cd.kind != kind {
            return Err("type");
        }
        if cd.origin != self.origin {
            return Err("origin");
        }
        let user = self
            .take_challenge(&cd.challenge, purpose)
            .ok_or("challenge")?;
        Ok((raw, user))
    }

    fn check_rp(&self, auth_data: &[u8]) -> Result<(u8, u32), &'static str> {
        if auth_data.len() < 37 {
            return Err("auth data");
        }
        if auth_data[..32] != Sha256::digest(self.rp_id.as_bytes())[..] {
            return Err("rp id");
        }
        let flags = auth_data[32];
        if flags & FLAG_UP == 0 {
            return Err("user presence");
        }
        let count = u32::from_be_bytes(auth_data[33..37].try_into().unwrap());
        Ok((flags, count))
    }

    /// 등록 응답을 확인한다. 성공하면 등록하는 사용자 id와 자격을 돌려준다.
    pub fn finish_registration(
        &self,
        res: &RegistrationResponse,
    ) -> Result<(i64, NewCredential), &'static str> {
        let (_, user) = self.check_client_data(
            &res.response.client_data_json,
            "webauthn.create",
            Purpose::Register,
        )?;
        let user = user.ok_or("user")?;
        let att = B64
            .decode(&res.response.attestation_object)
            .map_err(|_| "attestation")?;
        let att: Value = ciborium::de::from_reader(&att[..]).map_err(|_| "attestation")?;
        let auth_data = map_get(&att, "authData")
            .and_then(Value::as_bytes)
            .ok_or("auth data")?;
        let (flags, sign_count) = self.check_rp(auth_data)?;
        // 로그인에 UV가 필요하므로 등록 때 미리 확인한다(쓸 수 없는 패스키가 등록되지 않게).
        if flags & FLAG_UV == 0 {
            return Err("user verification");
        }
        if flags & FLAG_AT == 0 || auth_data.len() < 55 {
            return Err("credential data");
        }
        let id_len = u16::from_be_bytes([auth_data[53], auth_data[54]]) as usize;
        let id_end = 55 + id_len;
        if auth_data.len() <= id_end {
            return Err("credential data");
        }
        let cred_id = B64.encode(&auth_data[55..id_end]);
        if cred_id != res.id {
            return Err("credential id");
        }
        let cose: Value = ciborium::de::from_reader(&auth_data[id_end..]).map_err(|_| "key")?;
        let public_key = cose_es256(&cose).ok_or("key type")?;
        VerifyingKey::from_sec1_bytes(&public_key).map_err(|_| "key")?;
        Ok((
            user,
            NewCredential {
                id: cred_id,
                public_key,
                sign_count,
            },
        ))
    }

    /// 로그인 응답을 확인한다. `lookup`은 자격 id로 (사용자, 공개키, 저장된 서명 횟수)를 찾는다.
    /// 성공하면 (사용자, 새 서명 횟수).
    pub fn finish_login(
        &self,
        res: &AuthenticationResponse,
        lookup: impl FnOnce(&str) -> Option<(i64, Vec<u8>, u32)>,
    ) -> Result<(i64, u32), &'static str> {
        let (client_data, _) = self.check_client_data(
            &res.response.client_data_json,
            "webauthn.get",
            Purpose::Login,
        )?;
        let auth_data = B64
            .decode(&res.response.authenticator_data)
            .map_err(|_| "auth data")?;
        let (flags, count) = self.check_rp(&auth_data)?;
        if flags & FLAG_UV == 0 {
            return Err("user verification");
        }
        let (user, key, stored) = lookup(&res.id).ok_or("unknown credential")?;
        let key = VerifyingKey::from_sec1_bytes(&key).map_err(|_| "key")?;
        let sig = B64
            .decode(&res.response.signature)
            .map_err(|_| "signature")?;
        let sig = Signature::from_der(&sig).map_err(|_| "signature")?;
        let mut signed = auth_data.clone();
        signed.extend_from_slice(&Sha256::digest(&client_data));
        key.verify(&signed, &sig).map_err(|_| "signature")?;
        // 횟수를 쓰는 인증기에서 횟수가 늘지 않았으면 복제된 키일 수 있다.
        if (count != 0 || stored != 0) && count <= stored {
            return Err("sign count");
        }
        Ok((user, count))
    }
}

fn map_get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_map()?
        .iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .map(|(_, v)| v)
}

fn map_int(v: &Value, key: i64) -> Option<&Value> {
    v.as_map()?
        .iter()
        .find(|(k, _)| k.as_integer().map(i128::from) == Some(i128::from(key)))
        .map(|(_, v)| v)
}

/// COSE 키에서 ES256(kty=EC2, alg=-7, crv=P-256) 공개키를 SEC1 비압축 점으로.
fn cose_es256(cose: &Value) -> Option<Vec<u8>> {
    let int = |k| map_int(cose, k).and_then(Value::as_integer).map(i128::from);
    if int(1)? != 2 || int(3)? != -7 || int(-1)? != 1 {
        return None;
    }
    let x = map_int(cose, -2)?.as_bytes()?;
    let y = map_int(cose, -3)?.as_bytes()?;
    if x.len() != 32 || y.len() != 32 {
        return None;
    }
    let mut out = vec![0x04];
    out.extend_from_slice(x);
    out.extend_from_slice(y);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::SigningKey;
    use p256::ecdsa::signature::Signer;

    const ORIGIN: &str = "https://meet.example.com";

    fn auth_data(rp: &str, flags: u8, count: u32, cred: Option<(&[u8], &[u8])>) -> Vec<u8> {
        let mut d = Sha256::digest(rp.as_bytes()).to_vec();
        d.push(flags);
        d.extend_from_slice(&count.to_be_bytes());
        if let Some((id, cose)) = cred {
            d.extend_from_slice(&[0u8; 16]);
            d.extend_from_slice(&(id.len() as u16).to_be_bytes());
            d.extend_from_slice(id);
            d.extend_from_slice(cose);
        }
        d
    }

    fn cose_for(key: &SigningKey) -> Vec<u8> {
        let point = key.verifying_key().to_sec1_point(false);
        let bytes = point.as_bytes();
        let map = Value::Map(vec![
            (Value::Integer(1.into()), Value::Integer(2.into())),
            (Value::Integer(3.into()), Value::Integer((-7).into())),
            (Value::Integer((-1).into()), Value::Integer(1.into())),
            (
                Value::Integer((-2).into()),
                Value::Bytes(bytes[1..33].to_vec()),
            ),
            (
                Value::Integer((-3).into()),
                Value::Bytes(bytes[33..65].to_vec()),
            ),
        ]);
        let mut out = Vec::new();
        ciborium::ser::into_writer(&map, &mut out).unwrap();
        out
    }

    fn client_data(kind: &str, challenge: &str, origin: &str) -> String {
        B64.encode(json!({ "type": kind, "challenge": challenge, "origin": origin }).to_string())
    }

    #[test]
    fn parses_browser_field_names() {
        let reg: RegistrationResponse = serde_json::from_value(json!({
            "id": "abc",
            "response": { "clientDataJSON": "cd", "attestationObject": "ao" },
        }))
        .unwrap();
        assert_eq!(reg.response.client_data_json, "cd");
        let login: AuthenticationResponse = serde_json::from_value(json!({
            "id": "abc",
            "response": { "clientDataJSON": "cd", "authenticatorData": "ad", "signature": "s" },
        }))
        .unwrap();
        assert_eq!(login.response.client_data_json, "cd");
    }

    #[test]
    fn register_then_login_and_reject_tampering() {
        let wa = WebAuthn::new(ORIGIN);
        let key = {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).unwrap();
            seed[0] = 1; // 0 또는 곡선 차수 이상이 되지 않게
            SigningKey::from_slice(&seed).unwrap()
        };
        let cred_id = b"credential-1";

        let opts = wa.registration_options(7, "a@b.c", "A", &[]);
        let challenge = opts["challenge"].as_str().unwrap().to_owned();
        let ad = auth_data(
            "meet.example.com",
            FLAG_UP | FLAG_UV | FLAG_AT,
            0,
            Some((cred_id, &cose_for(&key))),
        );
        let att = Value::Map(vec![
            (Value::Text("fmt".into()), Value::Text("none".into())),
            (Value::Text("attStmt".into()), Value::Map(vec![])),
            (Value::Text("authData".into()), Value::Bytes(ad)),
        ]);
        let mut att_bytes = Vec::new();
        ciborium::ser::into_writer(&att, &mut att_bytes).unwrap();
        let reg = RegistrationResponse {
            id: B64.encode(cred_id),
            response: AttestationResponse {
                client_data_json: client_data("webauthn.create", &challenge, ORIGIN),
                attestation_object: B64.encode(&att_bytes),
            },
        };
        let (user, cred) = wa.finish_registration(&reg).unwrap();
        assert_eq!(user, 7);
        // 같은 챌린지는 다시 쓸 수 없다.
        assert!(wa.finish_registration(&reg).is_err());

        let login = |challenge: &str, origin: &str, count: u32| {
            let ad = auth_data("meet.example.com", FLAG_UP | FLAG_UV, count, None);
            let cd = client_data("webauthn.get", challenge, origin);
            let mut signed = ad.clone();
            signed.extend_from_slice(&Sha256::digest(B64.decode(&cd).unwrap()));
            let sig: Signature = key.sign(&signed);
            AuthenticationResponse {
                id: cred.id.clone(),
                response: AssertionResponse {
                    client_data_json: cd,
                    authenticator_data: B64.encode(ad),
                    signature: B64.encode(sig.to_der().as_bytes()),
                },
            }
        };
        let lookup = |_: &str| Some((7, cred.public_key.clone(), 0));
        let c1 = wa.login_options()["challenge"].as_str().unwrap().to_owned();
        assert_eq!(
            wa.finish_login(&login(&c1, ORIGIN, 5), lookup).unwrap(),
            (7, 5)
        );

        // 다른 사이트에서 받은 응답, 줄지 않은 서명 횟수, 모르는 챌린지는 거부한다.
        let c2 = wa.login_options()["challenge"].as_str().unwrap().to_owned();
        assert_eq!(
            wa.finish_login(&login(&c2, "https://evil.example", 6), lookup)
                .unwrap_err(),
            "origin"
        );
        let c3 = wa.login_options()["challenge"].as_str().unwrap().to_owned();
        assert_eq!(
            wa.finish_login(&login(&c3, ORIGIN, 5), |_| Some((
                7,
                cred.public_key.clone(),
                5
            )))
            .unwrap_err(),
            "sign count"
        );
        assert_eq!(
            wa.finish_login(&login("made-up", ORIGIN, 9), lookup)
                .unwrap_err(),
            "challenge"
        );
    }
}
