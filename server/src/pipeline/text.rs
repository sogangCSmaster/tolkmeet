//! STT 텍스트 다듬기와 문장 나누기. 상태 없는 순수 함수만 둔다.

/// 띄어쓰기 없이 쓰는 문자(가나·한자). 한글은 띄어 쓰므로 넣지 않는다.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{303F}' // CJK 구두점
        | '\u{3040}'..='\u{30FF}' // 히라가나·가타카나
        | '\u{31F0}'..='\u{31FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FF00}'..='\u{FFEF}' // 전각 구두점·문자
    )
}

/// 공백을 하나로 모으고, 일본어 중간 결과처럼 가나·한자 사이에 끼어든 공백은 지운다.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        out = join(&out, word);
    }
    out
}

/// 두 조각을 잇는다. 경계의 어느 한쪽이 가나·한자면 붙이고, 아니면 공백 하나를 둔다.
pub fn join(a: &str, b: &str) -> String {
    match (a.chars().next_back(), b.chars().next()) {
        (None, _) => b.to_owned(),
        (_, None) => a.to_owned(),
        (Some(x), Some(y)) if is_cjk(x) || is_cjk(y) => format!("{a}{b}"),
        _ => format!("{a} {b}"),
    }
}

/// 앞에서부터 `max`자(문자 경계)까지만 남긴다.
pub fn truncate_chars(mut text: String, max: usize) -> String {
    if let Some((end, _)) = text.char_indices().nth(max) {
        text.truncate(end);
    }
    text
}

/// 끝난 문장들과 아직 끝나지 않은 나머지로 나눈다.
/// `.`는 뒤가 공백이거나 끝일 때만 문장 끝으로 본다(`3.5` 같은 숫자를 자르지 않게).
pub fn split_sentences(text: &str) -> (Vec<String>, String) {
    // 문장 끝 부호 뒤에 바로 붙는 닫는 따옴표·괄호·부호는 그 문장에 포함한다.
    const TRAILING: &[char] = &[
        '」', '』', '"', '\'', ')', '）', '.', '?', '!', '。', '？', '！',
    ];
    let mut done = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next = chars.peek().map(|&(_, n)| n);
        let ends = match c {
            '。' | '？' | '！' => true,
            '.' | '?' | '!' => next.is_none_or(|n| n.is_whitespace() || TRAILING.contains(&n)),
            _ => false,
        };
        if !ends {
            continue;
        }
        let mut end = i + c.len_utf8();
        while let Some(&(j, n)) = chars.peek() {
            if !TRAILING.contains(&n) {
                break;
            }
            end = j + n.len_utf8();
            chars.next();
        }
        let sentence = text[start..end].trim();
        if !sentence.is_empty() {
            done.push(sentence.to_owned());
        }
        start = end;
    }
    (done, text[start..].trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_spaces_between_japanese_words_only() {
        assert_eq!(
            normalize(" 私たち の  投資 チーム は "),
            "私たちの投資チームは"
        );
        assert_eq!(normalize("저희  투자팀은\n주로"), "저희 투자팀은 주로");
        assert_eq!(normalize("主に AI に"), "主にAIに");
        assert_eq!(normalize("hello   world"), "hello world");
    }

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate_chars("가나다라".into(), 2), "가나");
        assert_eq!(truncate_chars("abc".into(), 5), "abc");
    }

    #[test]
    fn joins_with_space_only_between_spaced_scripts() {
        assert_eq!(join("저희는", "주로"), "저희는 주로");
        assert_eq!(join("主に", "AIに"), "主にAIに");
        assert_eq!(join("", "abc"), "abc");
        assert_eq!(join("abc", ""), "abc");
    }

    #[test]
    fn splits_finished_sentences() {
        assert_eq!(
            split_sentences("안녕하세요. 저는 유승재입니다. 오늘"),
            (
                vec!["안녕하세요.".into(), "저는 유승재입니다.".into()],
                "오늘".into()
            )
        );
        assert_eq!(
            split_sentences("はい。そうです。主にAI"),
            (vec!["はい。".into(), "そうです。".into()], "主にAI".into())
        );
        assert_eq!(
            split_sentences("Really?! Yes"),
            (vec!["Really?!".into()], "Yes".into())
        );
        assert_eq!(
            split_sentences("「はい。」と"),
            (vec!["「はい。」".into()], "と".into())
        );
    }

    #[test]
    fn does_not_split_decimals_or_unfinished_text() {
        assert_eq!(
            split_sentences("버전 3.5 모델"),
            (vec![], "버전 3.5 모델".into())
        );
        assert_eq!(
            split_sentences("Done."),
            (vec!["Done.".into()], String::new())
        );
        assert_eq!(split_sentences(""), (vec![], String::new()));
    }
}
