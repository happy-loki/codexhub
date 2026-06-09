pub(super) fn normalize_card_markdown(text: &str) -> String {
    sanitize_markdown_image_links(&text.replace("\r\n", "\n").trim())
}

fn sanitize_markdown_image_links(text: &str) -> String {
    let mut rest = text;
    let mut output = String::new();

    loop {
        let Some(start) = rest.find("![") else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..start]);

        let candidate = &rest[start..];
        let Some(alt_end) = candidate.find("](") else {
            output.push_str(candidate);
            break;
        };
        let target_start = alt_end + 2;
        let Some(target_end) = candidate[target_start..].find(')') else {
            output.push_str(candidate);
            break;
        };

        let alt = &candidate[2..alt_end];
        let target = &candidate[target_start..target_start + target_end];
        let full_end = target_start + target_end + 1;
        if is_feishu_image_key(target) {
            output.push_str(&candidate[..full_end]);
        } else {
            output.push_str(&markdown_image_text_replacement(alt, target));
        }
        rest = &candidate[full_end..];
    }

    output
}

fn is_feishu_image_key(target: &str) -> bool {
    target.trim().starts_with("img_")
}

fn markdown_image_text_replacement(alt: &str, target: &str) -> String {
    let alt = alt.trim();
    let target = target.trim().replace('`', "'");
    match (alt.is_empty(), target.is_empty()) {
        (true, true) => "图片".to_string(),
        (true, false) => format!("图片：`{target}`"),
        (false, true) => format!("图片：{alt}"),
        (false, false) => format!("图片：{alt}（`{target}`）"),
    }
}

pub fn normalize_cardkit_streaming_markdown(text: &str) -> String {
    normalize_card_markdown(text)
}

#[cfg(test)]
mod tests {
    use super::normalize_card_markdown;

    #[test]
    fn local_markdown_image_path_is_rendered_as_text() {
        let text =
            "看这里：![PNG 缩略图总览](C:/Users/me/AppData/Local/Temp/codex_png_contact_sheet.png)";

        let normalized = normalize_card_markdown(text);

        assert!(!normalized.contains("![PNG 缩略图总览]("));
        assert!(normalized.contains("图片：PNG 缩略图总览"));
        assert!(normalized.contains("codex_png_contact_sheet.png"));
    }

    #[test]
    fn feishu_image_key_markdown_is_preserved() {
        let text = "![预览](img_v3_02ad_abc)";

        let normalized = normalize_card_markdown(text);

        assert_eq!(normalized, text);
    }

    #[test]
    fn http_markdown_image_url_is_rendered_as_text() {
        let text = "![preview](https://example.com/a.png)";

        let normalized = normalize_card_markdown(text);

        assert_eq!(normalized, "图片：preview（`https://example.com/a.png`）");
    }
}
