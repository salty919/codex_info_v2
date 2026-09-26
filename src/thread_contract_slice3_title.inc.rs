// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

{
    fn title(item: &Value) -> String {
        validate_thread_item(item).unwrap().title().to_owned()
    }

    {
        let mut item = full_thread();
        item["name"] = json!("name");
        item["preview"] = json!("preview");
        assert_eq!(title(&item), "name", "title.name_wins");
    }
    {
        let mut item = full_thread();
        item["name"] = Value::Null;
        item["preview"] = json!("preview");
        assert_eq!(title(&item), "未設定", "title.null_name_is_unset");
    }
    {
        let mut item = full_thread();
        item["name"] = json!("");
        item["preview"] = json!("preview");
        assert_eq!(title(&item), "未設定", "title.empty_name_is_unset");
    }
    {
        let mut item = full_thread();
        item.as_object_mut().unwrap().remove("name");
        item["preview"] = json!("preview");
        assert_eq!(title(&item), "未設定", "title.absent_name_is_unset");
    }
    {
        let mut item = full_thread();
        item["name"] = json!("a\u{0000}\u{202e}   b");
        assert_eq!(title(&item), "a b", "title.name_normalized");
    }
    {
        let mut item = full_thread();
        item["name"] = Value::Null;
        item["preview"] = json!("p\n\u{2066}  q");
        assert_eq!(title(&item), "未設定", "title.preview_ignored");
    }
    {
        let mut item = full_thread();
        item["name"] = json!("");
        item["preview"] = json!("");
        item["title"] = json!("alias-title");
        item["threadTitle"] = json!("alias-thread-title");
        item["updated_at"] = json!("alias-updated-at");
        item["id"] = json!("different-valid-id");
        assert_eq!(
            title(&item),
            "未設定",
            "title.aliases_do_not_override"
        );
    }
    {
        let input = "界".repeat(512);
        assert_eq!(input.chars().count(), 512);
        let mut item = full_thread();
        item["name"] = json!(input.clone());
        let output = title(&item);
        assert_eq!(output, input, "title.name_512_preserved");
        assert_eq!(output.chars().count(), 512);
    }
    {
        let input = "界".repeat(513);
        let expected = format!("{}…", "界".repeat(511));
        assert_eq!(input.chars().count(), 513);
        assert_eq!(expected.chars().count(), 512);
        let mut item = full_thread();
        item["name"] = json!(input);
        let output = title(&item);
        assert_eq!(output, expected, "title.name_513_elided");
        assert_eq!(output.chars().count(), 512);
    }
    {
        let input = "語".repeat(512);
        assert_eq!(input.chars().count(), 512);
        let mut item = full_thread();
        item["name"] = Value::Null;
        item["preview"] = json!(input.clone());
        let output = title(&item);
        assert_eq!(output, "未設定", "title.preview_512_ignored");
    }
    {
        let input = "語".repeat(513);
        assert_eq!(input.chars().count(), 513);
        let mut item = full_thread();
        item["name"] = Value::Null;
        item["preview"] = json!(input);
        let output = title(&item);
        assert_eq!(output, "未設定", "title.preview_513_ignored");
    }

    let errors = [
        (ThreadContractError::InvalidCursor, "thread cursor rejected"),
        (ThreadContractError::InvalidRequest, "thread request rejected"),
        (
            ThreadContractError::InvalidEnvelope,
            "thread page envelope rejected",
        ),
        (ThreadContractError::InvalidItem, "thread item rejected"),
        (ThreadContractError::InvalidSchema, "thread schema rejected"),
        (
            ThreadContractError::ValidationBudgetExceeded,
            "thread validation budget exceeded",
        ),
        (
            ThreadContractError::InvalidManifest,
            "thread schema manifest rejected",
        ),
    ];
    for (error, expected) in errors {
        assert_eq!(error.message(), expected);
        assert_eq!(error.to_string(), expected);
    }

    let sentinel = "DO_NOT_LEAK_THREAD_VALUE";
    let nonleaking = [
        (
            validate_schema(&json!(sentinel), &json!(false)).unwrap_err(),
            ThreadContractError::InvalidItem,
        ),
        (
            validate_schema(&json!(sentinel), &json!(sentinel)).unwrap_err(),
            ThreadContractError::InvalidSchema,
        ),
        (
            validate_schema(
                &json!(sentinel),
                &json!({"$ref":"#", "x-vendor":sentinel}),
            )
            .unwrap_err(),
            ThreadContractError::ValidationBudgetExceeded,
        ),
    ];
    for (actual, expected) in nonleaking {
        assert_eq!(actual, expected);
        let displayed = actual.to_string();
        assert_eq!(displayed, expected.message());
        assert!(!displayed.contains(sentinel));
    }
}
