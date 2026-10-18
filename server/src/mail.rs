//! 메일 발송. `CLOUDFLARE_ACCOUNT_ID`와 `CLOUDFLARE_EMAIL_TOKEN`이 있으면 Cloudflare Email Sending REST API로 보내고,
//! 없으면 보내지 않는다.
//! 개발 모드에서는 키가 없을 때 `data/mail/`에 파일로 남겨 가입·재설정 흐름을 직접 확인할 수 있게 한다.
//! 메일 본문(인증 코드·재설정 링크)은 로그에 남기지 않는다.

use std::path::PathBuf;

use serde_json::json;

#[derive(Clone)]
pub struct Mailer {
    mode: Mode,
    from: String,
}

#[derive(Clone)]
enum Mode {
    Cloudflare {
        client: reqwest::Client,
        /// `.../accounts/{account_id}/email/sending/send`
        url: String,
        token: String,
    },
    /// 개발용: 파일로 떨어뜨린다.
    Folder(PathBuf),
    Off,
}

pub struct Message {
    pub to: String,
    pub subject: String,
    pub html: String,
    pub text: String,
}

impl Mailer {
    pub fn from_env(dev_folder: Option<PathBuf>) -> Self {
        // Cloudflare에 등록(Onboard)한 도메인의 주소여야 한다.
        let from = std::env::var("MAIL_FROM").unwrap_or_else(|_| "Tolkmeet <no-reply@tolkmeet.com>".into());
        let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        // 계정 id는 주소에 그대로 들어가므로 영숫자만 받는다.
        let account = var("CLOUDFLARE_ACCOUNT_ID").filter(|a| a.bytes().all(|b| b.is_ascii_alphanumeric()));
        let mode = match (account, var("CLOUDFLARE_EMAIL_TOKEN")) {
            (Some(account), Some(token)) => Mode::Cloudflare {
                client: reqwest::Client::new(),
                url: format!("https://api.cloudflare.com/client/v4/accounts/{account}/email/sending/send"),
                token,
            },
            _ => match dev_folder {
                Some(dir) => {
                    tracing::warn!(dir = %dir.display(), "CLOUDFLARE_ACCOUNT_ID/CLOUDFLARE_EMAIL_TOKEN not set: dev mode writes mail to files");
                    Mode::Folder(dir)
                }
                None => {
                    tracing::warn!(
                        "CLOUDFLARE_ACCOUNT_ID/CLOUDFLARE_EMAIL_TOKEN not set: emails (verification, password reset) are not sent"
                    );
                    Mode::Off
                }
            },
        };
        Self { mode, from }
    }

    #[cfg(test)]
    pub fn off() -> Self {
        Self {
            mode: Mode::Off,
            from: String::new(),
        }
    }

    /// 보내기는 요청을 붙잡지 않도록 따로 돌린다. 실패는 로그로만 남긴다(받는 주소는 남기지 않는다).
    pub fn send(&self, msg: Message) {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.deliver(&msg).await {
                tracing::warn!(error = %e, subject = %msg.subject, "mail not sent");
            }
        });
    }

    async fn deliver(&self, msg: &Message) -> Result<(), String> {
        match &self.mode {
            Mode::Cloudflare { client, url, token } => {
                let res = client
                    .post(url)
                    .bearer_auth(token)
                    .json(&json!({
                        "from": sender(&self.from),
                        "to": msg.to,
                        "subject": msg.subject,
                        "html": msg.html,
                        "text": msg.text,
                    }))
                    .timeout(std::time::Duration::from_secs(15))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let status = res.status();
                let body: serde_json::Value = res.json().await.unwrap_or_default();
                // 실패 이유는 Cloudflare 오류 코드·문구만 남긴다(받는 주소는 남기지 않는다).
                if !status.is_success() || body["success"] != true {
                    return Err(format!("cloudflare email status {status}: {}", body["errors"]));
                }
                if body["result"]["permanent_bounces"].as_array().is_some_and(|a| !a.is_empty()) {
                    return Err("cloudflare email: permanent bounce".into());
                }
                Ok(())
            }
            Mode::Folder(dir) => {
                tokio::fs::create_dir_all(dir)
                    .await
                    .map_err(|e| e.to_string())?;
                // 같은 초에 같은 주소로 두 통이 가도 덮어쓰지 않게 밀리초까지 넣는다.
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                let name = format!(
                    "{}-{}.txt",
                    millis,
                    msg.to.replace(|c: char| !c.is_ascii_alphanumeric(), "_")
                );
                let body = format!("To: {}\nSubject: {}\n\n{}\n", msg.to, msg.subject, msg.text);
                tokio::fs::write(dir.join(&name), body)
                    .await
                    .map_err(|e| e.to_string())?;
                // 실제로 보낼 HTML도 옆에 남겨 브라우저로 미리 볼 수 있게 한다.
                tokio::fs::write(dir.join(name.replace(".txt", ".html")), &msg.html)
                    .await
                    .map_err(|e| e.to_string())?;
                tracing::info!(file = %name, "dev mail written");
                Ok(())
            }
            Mode::Off => Err("mail is not configured".into()),
        }
    }
}

/// `MAIL_FROM`("이름 <주소>" 또는 주소만)을 Cloudflare가 받는 모양으로(이름이 있으면 {address, name}).
fn sender(from: &str) -> serde_json::Value {
    match from.trim().strip_suffix('>').and_then(|s| s.rsplit_once('<')) {
        Some((name, address)) if !name.trim().is_empty() => {
            json!({ "address": address.trim(), "name": name.trim().trim_matches('"') })
        }
        Some((_, address)) => json!(address.trim()),
        None => json!(from.trim()),
    }
}

/// 메일 문구(받는 사람이 가입할 때 쓰던 화면 언어로).
pub struct Copy {
    pub lang: Lang,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ko,
    Ja,
}

impl Lang {
    pub fn parse(s: &str) -> Self {
        match s {
            "ko" => Lang::Ko,
            "ja" => Lang::Ja,
            _ => Lang::En,
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 공통 틀: 로고, 제목, 본문 문단, (선택) 큰 코드 또는 버튼, 꼬리말.
fn layout(
    title: &str,
    paragraphs: &[String],
    code: Option<&str>,
    button: Option<(&str, &str)>,
    footer: &str,
) -> String {
    let mut body = String::new();
    for p in paragraphs {
        body.push_str(&format!(
            "<p style=\"margin:0 0 14px;font-size:15px;line-height:1.6;color:#424245\">{}</p>",
            escape(p)
        ));
    }
    if let Some(code) = code {
        body.push_str(&format!(
            "<p style=\"margin:8px 0 22px;font-size:34px;font-weight:700;letter-spacing:8px;color:#1D1D1F\">{}</p>",
            escape(code)
        ));
    }
    if let Some((label, url)) = button {
        body.push_str(&format!(
            "<p style=\"margin:8px 0 22px\"><a href=\"{}\" style=\"display:inline-block;padding:12px 22px;border-radius:22px;background:#D4401C;color:#FFFFFF;font-weight:700;text-decoration:none\">{}</a></p>",
            escape(url),
            escape(label)
        ));
    }
    format!(
        "<!doctype html><html><body style=\"margin:0;background:#F5F5F7;font-family:-apple-system,BlinkMacSystemFont,'Apple SD Gothic Neo','Hiragino Sans',sans-serif\">\
<div style=\"max-width:520px;margin:0 auto;padding:40px 24px\">\
<div style=\"font-size:18px;font-weight:800;letter-spacing:-0.5px;color:#1D1D1F\">\
<span style=\"display:inline-block;width:16px;height:4px;border-radius:2px;background:#1D1D1F;vertical-align:middle\"></span>\
<span style=\"display:inline-block;width:16px;height:4px;border-radius:2px;background:#F2542D;vertical-align:middle;margin:0 8px 0 -10px;position:relative;top:5px\"></span>tolkmeet</div>\
<div style=\"margin-top:28px;padding:32px;border-radius:22px;background:#FFFFFF\">\
<h1 style=\"margin:0 0 18px;font-size:24px;line-height:1.3;color:#1D1D1F\">{}</h1>{}</div>\
<p style=\"margin:18px 4px 0;font-size:12px;line-height:1.6;color:#6E6E73\">{}</p></div></body></html>",
        escape(title),
        body,
        escape(footer)
    )
}

fn text(title: &str, paragraphs: &[String], extra: &str, footer: &str) -> String {
    format!(
        "{title}\n\n{}\n\n{extra}\n\n{footer}\n",
        paragraphs.join("\n\n")
    )
}

impl Copy {
    fn footer(&self) -> &'static str {
        match self.lang {
            Lang::Ko => {
                "이 메일은 no-reply@tolkmeet.com에서 보냈습니다. 요청하지 않았다면 무시하셔도 됩니다."
            }
            Lang::Ja => {
                "このメールは no-reply@tolkmeet.com から送信されました。心当たりがない場合は無視してください。"
            }
            Lang::En => {
                "Sent from no-reply@tolkmeet.com. If you didn't request this, you can ignore it."
            }
        }
    }

    pub fn verify_code(&self, to: &str, code: &str, url: &str) -> Message {
        let (subject, title, lines, button) = match self.lang {
            Lang::Ko => (
                "Tolkmeet 인증 코드",
                "이메일 인증 코드",
                vec!["아래 6자리 코드를 가입 화면에 입력하거나 버튼을 누르세요. 15분 동안 쓸 수 있습니다.".to_owned()],
                "이메일 인증하기",
            ),
            Lang::Ja => (
                "Tolkmeet 認証コード",
                "メール認証コード",
                vec!["以下の6桁のコードを登録画面に入力するか、ボタンを押してください。15分間有効です。".to_owned()],
                "メールアドレスを確認",
            ),
            Lang::En => (
                "Your Tolkmeet code",
                "Your verification code",
                vec!["Enter this 6-digit code on the sign-up screen, or use the button. It expires in 15 minutes.".to_owned()],
                "Verify email",
            ),
        };
        Message {
            to: to.to_owned(),
            subject: format!("{subject}: {code}"),
            html: layout(
                title,
                &lines,
                Some(code),
                Some((button, url)),
                self.footer(),
            ),
            text: text(title, &lines, &format!("{code}\n{url}"), self.footer()),
        }
    }

    /// 이메일 주소를 바꿀 때 새 주소로 보내는 코드.
    pub fn email_change_code(&self, to: &str, code: &str) -> Message {
        let (subject, title, lines) = match self.lang {
            Lang::Ko => ("Tolkmeet 이메일 변경 코드", "새 이메일 확인", vec!["계정 화면에 아래 코드를 입력하면 이 주소로 바뀝니다. 15분 동안 쓸 수 있습니다.".to_owned()]),
            Lang::Ja => ("Tolkmeet メールアドレス変更コード", "新しいメールアドレスの確認", vec!["アカウント画面に以下のコードを入力すると、このアドレスに変更されます。15分間有効です。".to_owned()]),
            Lang::En => ("Confirm your new Tolkmeet email", "Confirm your new email", vec!["Enter this code on the account screen to switch to this address. It expires in 15 minutes.".to_owned()]),
        };
        Message {
            to: to.to_owned(),
            subject: format!("{subject}: {code}"),
            html: layout(title, &lines, Some(code), None, self.footer()),
            text: text(title, &lines, code, self.footer()),
        }
    }

    /// 이메일이 바뀌었다는 알림(예전 주소로).
    pub fn email_changed_notice(&self, old: &str, new: &str) -> Message {
        let (subject, title, lines) = match self.lang {
            Lang::Ko => (
                "Tolkmeet 계정 이메일이 바뀌었습니다",
                "계정 이메일이 바뀌었습니다",
                vec![
                    format!("이 계정의 로그인 이메일이 {new}(으)로 바뀌었습니다."),
                    "직접 바꾸지 않았다면 support@tolkmeet.com으로 바로 알려 주세요.".to_owned(),
                ],
            ),
            Lang::Ja => (
                "Tolkmeet アカウントのメールアドレスが変更されました",
                "メールアドレスが変更されました",
                vec![
                    format!("このアカウントのログイン用メールアドレスが {new} に変更されました。"),
                    "心当たりがない場合は、すぐに support@tolkmeet.com までご連絡ください。"
                        .to_owned(),
                ],
            ),
            Lang::En => (
                "Your Tolkmeet email was changed",
                "Your account email was changed",
                vec![
                    format!("The sign-in email for this account is now {new}."),
                    "If you didn't do this, contact support@tolkmeet.com right away.".to_owned(),
                ],
            ),
        };
        Message {
            to: old.to_owned(),
            subject: subject.to_owned(),
            html: layout(title, &lines, None, None, self.footer()),
            text: text(title, &lines, "", self.footer()),
        }
    }

    pub fn reset_link(&self, to: &str, url: &str) -> Message {
        let (subject, title, lines, button) = match self.lang {
            Lang::Ko => ("Tolkmeet 비밀번호 재설정", "비밀번호 재설정", vec!["아래 버튼을 눌러 새 비밀번호를 만드세요. 링크는 30분 동안, 한 번만 쓸 수 있습니다.".to_owned()], "새 비밀번호 만들기"),
            Lang::Ja => ("Tolkmeet パスワードの再設定", "パスワードの再設定", vec!["下のボタンから新しいパスワードを設定してください。リンクは30分間、1回だけ使えます。".to_owned()], "新しいパスワードを設定"),
            Lang::En => ("Reset your Tolkmeet password", "Reset your password", vec!["Use the button below to set a new password. The link works once and expires in 30 minutes.".to_owned()], "Set a new password"),
        };
        Message {
            to: to.to_owned(),
            subject: subject.to_owned(),
            html: layout(title, &lines, None, Some((button, url)), self.footer()),
            text: text(title, &lines, url, self.footer()),
        }
    }

    pub fn invite(&self, to: &str, host: &str, title: &str, when: &str, url: &str) -> Message {
        let (subject, heading, lines, button) = match self.lang {
            Lang::Ko => (
                format!("{host}님이 회의에 초대했습니다: {title}"),
                title.to_owned(),
                vec![format!("{host}님이 Tolkmeet 회의에 초대했습니다."), when.to_owned(), "가입이나 설치 없이 링크를 열고 자기 언어를 고르면 됩니다. 말하는 대로 상대의 언어로 자막이 나옵니다.".to_owned()],
                "회의 참여",
            ),
            Lang::Ja => (
                format!("{host}さんから会議への招待: {title}"),
                title.to_owned(),
                vec![format!("{host}さんが Tolkmeet の会議に招待しています。"), when.to_owned(), "登録やインストールは不要です。リンクを開いて自分の言語を選ぶだけで、話した内容が相手の言語の字幕になります。".to_owned()],
                "会議に参加",
            ),
            Lang::En => (
                format!("{host} invited you to a meeting: {title}"),
                title.to_owned(),
                vec![format!("{host} invited you to a Tolkmeet meeting."), when.to_owned(), "No sign-up or install. Open the link and pick your language; what everyone says appears as captions in yours.".to_owned()],
                "Join meeting",
            ),
        };
        Message {
            to: to.to_owned(),
            subject,
            html: layout(&heading, &lines, None, Some((button, url)), self.footer()),
            text: text(&heading, &lines, url, self.footer()),
        }
    }

    pub fn receipt(&self, to: &str, plan: &str, amount: &str, renews: &str) -> Message {
        let (subject, title, lines) = match self.lang {
            Lang::Ko => (format!("Tolkmeet {plan} 요금제를 시작해 주셔서 감사합니다"), format!("{plan} 요금제를 시작해 주셔서 감사합니다"), vec![format!("결제 금액: {amount}"), format!("다음 결제일: {renews}"), "세금 영수증은 판매 대행사 Paddle이 따로 보냅니다. 해지와 영수증은 계정 › 요금제와 결제에서 할 수 있습니다.".to_owned()]),
            Lang::Ja => (format!("Tolkmeet {plan} プランをご利用いただきありがとうございます"), format!("{plan} プランをご利用いただきありがとうございます"), vec![format!("お支払い金額: {amount}"), format!("次回のお支払い日: {renews}"), "税務上の領収書は販売代理店の Paddle から別途届きます。解約と領収書はアカウント › プランとお支払いから確認できます。".to_owned()]),
            Lang::En => (format!("Thanks for starting Tolkmeet {plan}"), format!("Thanks for starting {plan}"), vec![format!("Amount: {amount}"), format!("Next payment: {renews}"), "Paddle, our reseller, sends the tax receipt separately. Cancel or see receipts in Account › Plan & billing.".to_owned()]),
        };
        Message {
            to: to.to_owned(),
            subject,
            html: layout(&title, &lines, None, None, self.footer()),
            text: text(&title, &lines, "", self.footer()),
        }
    }

    /// 운영자에게: 오늘(UTC) 추정 지출이 상한을 넘어 자막을 멈췄다. `spent`·`limit`은 `$12.34` 꼴.
    pub fn spend_alert(&self, to: &str, day: &str, spent: &str, limit: &str) -> Message {
        let (subject, title, lines) = match self.lang {
            Lang::Ko => (format!("Tolkmeet 하루 지출 상한 도달 ({day})"), "자막을 멈췄습니다".to_owned(), vec![format!("{day}(UTC) 추정 Gemini 지출 {spent}가 상한 {limit}을 넘어 모든 회의의 음성 인식·번역을 멈췄습니다. 영상·음성은 그대로입니다."), "다음 UTC 날짜(한국 시간 오전 9시)에 저절로 풀립니다. 더 일찍 풀려면 DAILY_SPEND_LIMIT_USD를 올리고 서버를 다시 시작하세요.".to_owned()]),
            Lang::Ja => (format!("Tolkmeet 1日の支出上限に到達 ({day})"), "字幕を停止しました".to_owned(), vec![format!("{day}(UTC)の推定 Gemini 支出 {spent} が上限 {limit} を超えたため、すべての会議の音声認識・翻訳を停止しました。映像・音声はそのままです。"), "次の UTC 日付(日本時間午前9時)に自動で再開します。早く再開するには DAILY_SPEND_LIMIT_USD を上げてサーバーを再起動してください。".to_owned()]),
            Lang::En => (format!("Tolkmeet daily spend limit reached ({day})"), "Captions are paused".to_owned(), vec![format!("Estimated Gemini spend for {day} (UTC) is {spent}, over the {limit} limit, so speech recognition and translation are paused in every meeting. Video and audio keep going."), "They come back on their own at the next UTC day. To resume sooner, raise DAILY_SPEND_LIMIT_USD and restart the server.".to_owned()]),
        };
        Message {
            to: to.to_owned(),
            subject,
            html: layout(&title, &lines, None, None, self.footer()),
            text: text(&title, &lines, "", self.footer()),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sender_is_sent_in_the_shape_cloudflare_takes() {
        assert_eq!(
            sender("Tolkmeet <no-reply@tolkmeet.com>"),
            serde_json::json!({ "address": "no-reply@tolkmeet.com", "name": "Tolkmeet" })
        );
        assert_eq!(sender("\"Tolk Meet\" <a@b.co>")["name"], "Tolk Meet");
        assert_eq!(sender("no-reply@tolkmeet.com"), serde_json::json!("no-reply@tolkmeet.com"));
        assert_eq!(sender("<a@b.co>"), serde_json::json!("a@b.co"));
    }

    use super::*;

    #[test]
    fn user_text_is_escaped_in_html() {
        let msg = Copy { lang: Lang::En }.invite(
            "a@b.c",
            "<b>Eve</b>",
            "Q&A",
            "Mon",
            "https://x/abcd-efgh-ijkm",
        );
        assert!(msg.html.contains("&lt;b&gt;Eve&lt;/b&gt;"));
        assert!(!msg.html.contains("<b>Eve</b>"));
        assert!(msg.html.contains("Q&amp;A"));
    }

    #[test]
    fn code_mail_is_in_the_users_language() {
        assert!(
            Copy { lang: Lang::Ko }
                .verify_code("a@b.c", "123456", "https://x")
                .subject
                .contains("인증 코드")
        );
        assert!(
            Copy { lang: Lang::Ja }
                .verify_code("a@b.c", "123456", "https://x")
                .subject
                .contains("認証コード")
        );
    }
}
