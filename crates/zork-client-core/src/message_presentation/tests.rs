use super::*;
use serde_json::json;
use zork_client_types::chat::{MessageQuote, QuoteKind};
use zork_client_types::comments::{CommentSource, DraftComment};

/// Saturday 2026-09-26 14:30:00 in UTC+8.
fn now() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-09-26T14:30:00+08:00").unwrap()
}
fn at(minutes_ago: i64) -> String {
    (now() - Duration::minutes(minutes_ago)).to_rfc3339()
}
fn options() -> PresentOptions {
    PresentOptions {
        now: now(),
        locale: TimeLocale::ZhCn,
        has_older: false,
        devices: HashMap::new(),
        agent_models: HashMap::new(),
    }
}
fn agent(id: &str, name: &str, minutes_ago: i64, text: &str) -> TranscriptLine {
    TranscriptLine::Message {
        role: Role::Assistant,
        content: text.into(),
        metadata: MessageMetadata {
            id: Some(format!("{id}-{minutes_ago}")),
            created_at: Some(at(minutes_ago)),
            author_agent_id: Some(id.into()),
            author_name: Some(name.into()),
            ..Default::default()
        },
    }
}
fn user(id: &str, minutes_ago: i64, text: &str) -> TranscriptLine {
    TranscriptLine::Message {
        role: Role::User,
        content: text.into(),
        metadata: MessageMetadata {
            id: Some(id.into()),
            created_at: Some(at(minutes_ago)),
            ..Default::default()
        },
    }
}
fn with(mut line: TranscriptLine, edit: impl FnOnce(&mut MessageMetadata)) -> TranscriptLine {
    let TranscriptLine::Message { metadata, .. } = &mut line;
    edit(metadata);
    line
}
fn reply(line: TranscriptLine, to: &str, quote: Option<(&str, QuoteKind)>) -> TranscriptLine {
    with(line, |m| {
        m.reply_to = Some(to.into());
        m.quote = quote.map(|(text, _)| text.into());
        m.quote_kind = quote.map(|(_, kind)| kind);
    })
}
fn run(lines: &[TranscriptLine], options: &PresentOptions) -> TranscriptPresentation {
    present(lines.iter().map(PresentRow::from), options)
}

#[test]
fn preferred_slots_match_the_prototype() {
    // Vectors computed with the prototype's JavaScript `tintSlots`.
    for (id, slot) in [
        ("planner", 4),
        ("builder", 1),
        ("review", 3),
        ("tester", 3),
        ("docs", 0),
        ("ops", 3),
        ("design", 0),
        ("审阅助手", 4),
        ("key:abc/worker", 2),
    ] {
        assert_eq!(preferred_slot(id), slot, "{id}");
    }
}

#[test]
fn tint_collisions_take_the_next_free_slot_in_first_appearance_order() {
    let three = TintSlots::assign(["planner", "builder", "review"]);
    assert_eq!(
        three.entries(),
        [
            ("planner".into(), 4),
            ("builder".into(), 1),
            ("review".into(), 3)
        ]
    );
    let seven = TintSlots::assign([
        "planner", "builder", "review", "tester", "docs", "ops", "design", "planner",
    ]);
    let slots: Vec<usize> = seven.entries().iter().map(|(_, slot)| *slot).collect();
    // tester prefers 3 (taken) → 4 (taken) → 0; docs prefers 0 → 1 → 2; the
    // sixth and seventh keep their preference once all five are used.
    assert_eq!(slots, [4, 1, 3, 0, 2, 3, 0]);
    let mut first_five = slots[..5].to_vec();
    first_five.sort();
    assert_eq!(first_five, [0, 1, 2, 3, 4]);
    // Order matters: the first agent always keeps its preferred slot.
    let reordered = TintSlots::assign([
        "docs", "ops", "design", "planner", "builder", "review", "tester",
    ]);
    let slots: Vec<usize> = reordered.entries().iter().map(|(_, slot)| *slot).collect();
    assert_eq!(slots, [0, 3, 1, 4, 2, 3, 3]);
    // An agent not in the Chat uses its preferred slot.
    assert_eq!(three.slot("docs"), 0);
    assert_eq!(three.slot("review"), 3);
}

#[test]
fn initials_uppercase_latin_and_keep_the_first_cjk_character() {
    assert_eq!(initial("planner"), "P");
    assert_eq!(initial("  builder"), "B");
    assert_eq!(initial("审阅助手"), "审");
    assert_eq!(initial("émile"), "é");
    assert_eq!(initial("3d"), "3");
    assert_eq!(initial(""), "?");
}

#[test]
fn groups_join_the_same_author_within_five_minutes() {
    let lines = vec![
        agent("planner", "Planner", 30, "a"),
        agent("planner", "Planner", 25, "exactly five minutes later"),
        with(
            agent("planner", "Planner", 25, "five minutes and a second"),
            |m| {
                m.id = Some("late".into());
                m.created_at =
                    Some((now() - Duration::minutes(20) + Duration::seconds(1)).to_rfc3339());
            },
        ),
        agent("builder", "Builder", 19, "other author"),
        user("u1", 18, "me"),
        user("u2", 17, "me again"),
        with(user("pending", 0, "sending"), |m| m.created_at = None),
        agent("builder", "Builder", 16, "back"),
    ];
    let shown = run(&lines, &options());
    let heads: Vec<bool> = shown.rows.iter().map(|r| r.group_head).collect();
    assert_eq!(heads, [true, false, true, true, true, false, false, true]);
    let tails: Vec<bool> = shown.rows.iter().map(|r| r.group_tail).collect();
    assert_eq!(tails, [false, true, true, true, false, false, true, true]);
    let placements: Vec<Option<TimePlacement>> = shown
        .rows
        .iter()
        .map(|r| r.time.as_ref().map(|t| t.placement))
        .collect();
    use TimePlacement::*;
    assert_eq!(
        placements,
        [
            Some(Head),
            Some(Hover),
            Some(Head),
            Some(Head),
            Some(Hover),
            Some(Hover),
            // A pending row has no time yet; its group shows none until sent.
            None,
            Some(Head),
        ]
    );
    // Identity only at agent group heads.
    let identities: Vec<bool> = shown.rows.iter().map(|r| r.identity.is_some()).collect();
    assert_eq!(
        identities,
        [true, false, true, true, false, false, false, true]
    );
    assert_eq!(shown.rows[0].time.as_ref().unwrap().label, "30 分钟前");
    assert_eq!(
        shown.rows[0].time.as_ref().unwrap().full,
        "2026年9月26日 周六 14:00:00"
    );
    // The earliest change among labels: every minute label moves within 60 s.
    assert!(shown.next_change_ms.is_some_and(|ms| ms <= 60_000));
}

#[test]
fn a_user_group_puts_its_time_under_the_last_bubble() {
    let shown = run(&[user("a", 3, "one"), user("b", 2, "two")], &options());
    assert_eq!(
        shown.rows[0].time.as_ref().unwrap().placement,
        TimePlacement::Hover
    );
    let last = shown.rows[1].time.as_ref().unwrap();
    assert_eq!(
        (last.placement, last.label.as_str()),
        (TimePlacement::Tail, "2 分钟前")
    );
    assert!(shown.rows.iter().all(|r| r.identity.is_none()));
}

#[test]
fn short_originals_are_quoted_verbatim_by_character_count() {
    // Ten CJK characters (30 bytes) are short; eleven are not.
    let ten = "一二三四五六七八九十";
    let content = quote_content(ten, None);
    assert_eq!(
        (content.source, content.text.as_str()),
        (QuoteSource::Original, ten)
    );
    assert_eq!((content.tooltip, content.mark), (None, None));
    let eleven = "一二三四五六七八九十一";
    assert_eq!(quote_content(eleven, None).source, QuoteSource::Fallback);
    // Surrounding whitespace does not count; a short original even wins over
    // a quote the agent supplied.
    let excerpt = MessageQuote {
        text: "好".into(),
        kind: QuoteKind::Excerpt,
    };
    let content = quote_content("  好。\n", Some(&excerpt));
    assert_eq!(
        (content.source, content.text.as_str()),
        (QuoteSource::Original, "好。")
    );
    assert_eq!(
        quote_content("0123456789", None).source,
        QuoteSource::Original
    );
    assert_eq!(
        quote_content("0123456789a", None).source,
        QuoteSource::Fallback
    );
}

#[test]
fn longer_originals_use_the_agent_quote_or_a_legacy_excerpt() {
    let original = "看过了：密码框缺少显示/隐藏切换，错误提示的对比度也不够。";
    let excerpt = MessageQuote {
        text: " 密码框缺少显示/隐藏切换 ".into(),
        kind: QuoteKind::Excerpt,
    };
    let content = quote_content(original, Some(&excerpt));
    assert_eq!(content.source, QuoteSource::Excerpt);
    assert_eq!(content.text, "密码框缺少显示/隐藏切换");
    assert_eq!(content.mark.as_deref(), Some("密码框缺少显示/隐藏切换"));
    assert_eq!(content.tooltip.as_deref(), Some("密码框缺少显示/隐藏切换"));
    let summary = MessageQuote {
        text: "缺切换，\n对比度不够".into(),
        kind: QuoteKind::Summary,
    };
    let content = quote_content(original, Some(&summary));
    assert_eq!(content.source, QuoteSource::Summary);
    assert_eq!(content.text, "缺切换， 对比度不够");
    assert_eq!(content.mark, None);
    // Legacy: the start of the original, numbered lists joined by " · ".
    let content = quote_content(
        "好的，拆成三步：\n1. 梳理流程\n2. 出新版布局\n\n3. 实现",
        None,
    );
    assert_eq!(content.source, QuoteSource::Fallback);
    assert_eq!(
        content.text,
        "好的，拆成三步： · 梳理流程 · 出新版布局 · 实现"
    );
    assert_eq!(content.tooltip.as_deref(), Some(content.text.as_str()));
    assert_eq!(content.mark, None);
    let long = "长".repeat(500);
    assert_eq!(fallback_excerpt(&long).chars().count(), 120);
}

#[test]
fn only_own_messages_in_between_is_the_core_part_of_the_omission_rule() {
    let authors: Vec<String> = ["a", "b", "b", "b", "a", "b"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(only_own_between(&authors, 0, 1)); // right after
    assert!(only_own_between(&authors, 0, 3)); // own messages in between
    assert!(!only_own_between(&authors, 0, 5)); // "a" in between
    assert!(only_own_between(&authors, 4, 5));
    assert!(!only_own_between(&authors, 3, 3));
    assert!(!only_own_between(&authors, 5, 4)); // target after reply
    assert!(!only_own_between(&authors, 0, 9));

    let lines = vec![
        user(
            "ask",
            30,
            "把登录页改版拆成任务，审阅助手和 Builder 一起跟进。",
        ),
        agent("builder", "Builder", 29, "骨架已推到分支。"),
        reply(agent("builder", "Builder", 28, "补一句"), "ask", None),
        agent("review", "审阅助手", 27, "看过了"),
        reply(
            agent("builder", "Builder", 26, "回到最初的要求"),
            "ask",
            None,
        ),
    ];
    let shown = run(&lines, &options());
    let line = shown.rows[2].reply.as_ref().unwrap();
    assert!(line.own_run, "only Builder's own message lies in between");
    assert_eq!(line.placement, ReplyPlacement::AboveBody);
    let line = shown.rows[4].reply.as_ref().unwrap();
    assert!(!line.own_run, "审阅助手 spoke in between");
    assert_eq!(line.placement, ReplyPlacement::InHead);
}

#[test]
fn reply_lines_resolve_targets_quotes_and_missing_originals() {
    let lines = vec![
        agent(
            "review",
            "审阅助手",
            40,
            "看过了：密码框缺少显示/隐藏切换，错误提示的对比度也不够。",
        ),
        user("short", 39, "好。"),
        reply(
            agent("builder", "Builder", 38, "已改"),
            "review-40",
            Some(("密码框缺少显示/隐藏切换", QuoteKind::Summary)),
        ),
        reply(agent("builder", "Builder", 37, "收到"), "short", None),
        reply(
            agent("planner", "Planner", 10, "按旧结论执行"),
            "gone",
            None,
        ),
    ];
    let shown = run(&lines, &options());
    let line = shown.rows[2].reply.as_ref().unwrap();
    assert_eq!(line.state, TargetState::Linked);
    assert_eq!(line.target_index, Some(0));
    let target = line.target.as_ref().unwrap();
    assert_eq!(
        (target.name.as_str(), target.initial.as_deref()),
        ("审阅助手", Some("审"))
    );
    assert_eq!(target.tint, Some(shown.tints.slot("review")));
    assert_eq!(line.content.as_ref().unwrap().source, QuoteSource::Summary);
    // A reply to the user's short message quotes it verbatim; the user has no disc.
    let line = shown.rows[3].reply.as_ref().unwrap();
    let target = line.target.as_ref().unwrap();
    assert_eq!(
        (target.name.as_str(), target.user, target.tint),
        ("你", true, None)
    );
    assert_eq!(line.content.as_ref().unwrap().text, "好。");
    // Missing: deleted without older history, not loaded with it.
    let line = shown.rows[4].reply.as_ref().unwrap();
    assert_eq!(
        (line.state, line.target.clone(), line.content.clone()),
        (TargetState::Deleted, None, None)
    );
    assert!(!line.own_run);
    let older = PresentOptions {
        has_older: true,
        ..options()
    };
    let shown = run(&lines, &older);
    assert_eq!(
        shown.rows[4].reply.as_ref().unwrap().state,
        TargetState::NotLoaded
    );
}

#[test]
fn comment_batches_become_pairs_with_resolved_sources() {
    let source =
        |message_id: Option<&str>, author: &str, agent: Option<&str>, quote: &str| CommentSource {
            session_id: "chat".into(),
            message_id: message_id.map(str::to_owned),
            author: Some(author.into()),
            author_agent_id: agent.map(str::to_owned),
            quote: quote.into(),
        };
    let payload = comments::compose(
        "其他都可以，按这个继续。",
        &[
            DraftComment {
                id: "d1".into(),
                source: source(
                    Some("planner-50"),
                    "Planner",
                    Some("planner"),
                    "错误提示贴在对应字段下方",
                ),
                comment: "这条保留".into(),
            },
            DraftComment {
                id: "d2".into(),
                source: source(Some("older"), "审阅助手", Some("review"), "建议下移 8 px"),
                comment: "试试 12 px".into(),
            },
            DraftComment {
                id: "d3".into(),
                source: CommentSource {
                    quote: "旧草稿".into(),
                    ..Default::default()
                },
                comment: "旧客户端".into(),
            },
        ],
    );
    let lines = vec![
        agent(
            "planner",
            "Planner",
            50,
            "验收标准：错误提示贴在对应字段下方，不弹窗。",
        ),
        user("batch", 20, &payload),
        reply(
            agent("builder", "Builder", 14, "两处都按你的意见改"),
            "batch",
            Some(("错误文案要具体；链接下移 12 px", QuoteKind::Summary)),
        ),
    ];
    let shown = run(
        &lines,
        &PresentOptions {
            has_older: true,
            ..options()
        },
    );
    let view = shown.rows[1].comments.as_ref().unwrap();
    assert_eq!(view.extra_text, "其他都可以，按这个继续。");
    assert_eq!(view.pairs.len(), 3);
    let first = &view.pairs[0];
    assert_eq!(
        (first.state, first.source_index),
        (TargetState::Linked, Some(0))
    );
    assert_eq!(first.source.tint, Some(shown.tints.slot("planner")));
    assert_eq!(first.pair.quote, "错误提示贴在对应字段下方");
    assert_eq!(first.pair.reply, "这条保留");
    let second = &view.pairs[1];
    assert_eq!(second.state, TargetState::NotLoaded);
    assert_eq!(second.source.name, "审阅助手");
    assert_eq!(second.source.tint, Some(preferred_slot("review")));
    let legacy = &view.pairs[2];
    assert_eq!(legacy.state, TargetState::Deleted);
    assert_eq!(
        (legacy.source.name.as_str(), legacy.source.tint),
        ("消息", None)
    );
    // A reply to a batch measures and quotes its replies, not the envelope.
    let line = shown.rows[2].reply.as_ref().unwrap();
    assert_eq!(line.content.as_ref().unwrap().source, QuoteSource::Summary);
    assert!(shown.rows[0].comments.is_none());
}

#[test]
fn identity_details_and_device_display() {
    let mut options = options();
    options.devices.insert(
        "key:studio".into(),
        DeviceLabel {
            display: "B".into(),
            machine: Some("zuozijians-Mac-Studio".into()),
        },
    );
    let lines = vec![
        with(agent("builder", "Builder", 10, "a"), |m| {
            m.device = Some("key:studio".into());
            m.model = Some("deepseek-flash · high".into());
        }),
        with(agent("planner", "Planner", 5, "b"), |m| {
            m.device = Some("key:air".into())
        }),
    ];
    let shown = run(&lines, &options);
    assert!(shown.multi_device);
    let identity = shown.rows[0].identity.as_ref().unwrap();
    assert_eq!(
        identity.detail,
        "Builder · B（zuozijians-Mac-Studio） · deepseek-flash · high"
    );
    assert_eq!(identity.device_name.as_deref(), Some("B"));
    let identity = shown.rows[1].identity.as_ref().unwrap();
    assert_eq!(identity.detail, "Planner");
    assert_eq!(identity.author.initial.as_deref(), Some("P"));
    // One device: no device names after agent names.
    assert!(!run(&lines[..1], &options).multi_device);
}

#[test]
fn bridge_accepts_conversation_rows_as_observed() {
    let rows = json!([
        {"type":"message","role":"assistant","content":"验收标准：首屏只保留账号、密码和一个主按钮。","id":"m1",
         "created_at":"2026-09-26T06:00:00Z","author_agent_id":"planner","author_name":"Planner",
         "display_content":"ignored","file_views":[],"pending":false},
        {"type":"message","role":"assistant","content":"复查通过","id":"m2","created_at":"2026-09-26T06:27:00Z",
         "author_agent_id":"review","author_name":"审阅助手","reply_to":"m1","quote":"首屏只保留账号、密码和一个主按钮",
         "quote_kind":"excerpt"}
    ]);
    let request: PresentRequest = serde_json::from_value(json!({
        "rows": rows, "now_ms": now().timestamp_millis(), "utc_offset_minutes": 480,
        "locale": "zh-CN", "has_older": false
    }))
    .unwrap();
    let value = handle(request).unwrap();
    assert_eq!(value["rows"][0]["time"]["label"], "30 分钟前");
    assert_eq!(value["rows"][0]["identity"]["name"], "Planner");
    assert_eq!(value["rows"][0]["identity"]["initial"], "P");
    assert_eq!(value["rows"][1]["reply"]["state"], "linked");
    assert_eq!(value["rows"][1]["reply"]["content"]["source"], "excerpt");
    assert_eq!(
        value["rows"][1]["reply"]["content"]["mark"],
        "首屏只保留账号、密码和一个主按钮"
    );
    // Right after its original: the UI omits the line if it is within a screen.
    assert_eq!(value["rows"][1]["reply"]["own_run"], true);
    assert_eq!(value["rows"][1]["reply"]["placement"], "in_head");
    assert_eq!(value["rows"][1]["time"]["placement"], "head");
    assert_eq!(value["multi_device"], false);
    assert!(value["next_change_ms"].as_i64().unwrap() <= 60_000);
    let english: PresentRequest = serde_json::from_value(json!({
        "rows": [], "now_ms": 0, "utc_offset_minutes": 0, "locale": "en-US"
    }))
    .unwrap();
    assert_eq!(handle(english).unwrap()["rows"], json!([]));
    let bad: PresentRequest = serde_json::from_value(json!({
        "rows": [], "now_ms": 0, "utc_offset_minutes": 100000
    }))
    .unwrap();
    assert!(handle(bad).is_err());
}

#[test]
fn maker_marks_resolve_per_model_with_generic_and_initial_fallbacks() {
    assert_eq!(
        maker_key(Some("claude-sonnet-5")).as_deref(),
        Some("anthropic")
    );
    assert_eq!(maker_key(Some("gpt-6-astra")).as_deref(), Some("openai"));
    assert_eq!(
        maker_key(Some("deepseek-flash")).as_deref(),
        Some("deepseek")
    );
    assert_eq!(
        maker_key(Some("moonshotai/kimi-k2.6")).as_deref(),
        Some("moonshot")
    );
    // A model the catalog does not know still gets a mark: generic.
    assert_eq!(maker_key(Some("house-model-7")).as_deref(), Some("generic"));
    // No model: the disc shows the initial.
    assert_eq!(maker_key(None), None);
    assert_eq!(maker_key(Some("  ")), None);
}

#[test]
fn agent_identities_show_the_maker_of_the_model_that_wrote_the_row() {
    let lines = vec![
        with(agent("builder", "Builder", 30, "a"), |m| {
            m.model = Some("glm-5.1".into())
        }),
        // An older Station or remote author: no model on the row.
        agent("review", "审阅助手", 20, "b"),
        agent("tester", "Tester", 10, "c"),
    ];
    let mut options = options();
    options
        .agent_models
        .insert("review".into(), "claude-sonnet-5".into());
    let shown = run(&lines, &options);
    let maker = |at: usize| {
        shown.rows[at]
            .identity
            .as_ref()
            .unwrap()
            .author
            .maker
            .clone()
    };
    assert_eq!(maker(0).as_deref(), Some("zhipu"));
    // Falls back to the Agent's known current model.
    assert_eq!(maker(1).as_deref(), Some("anthropic"));
    // Nothing known: the initial stays.
    assert_eq!(maker(2), None);
    assert_eq!(
        shown.rows[2]
            .identity
            .as_ref()
            .unwrap()
            .author
            .initial
            .as_deref(),
        Some("T")
    );
    // The same agent's history keeps the maker of the model it used then.
    let switched = vec![
        with(agent("builder", "Builder", 30, "old"), |m| {
            m.model = Some("gpt-5".into())
        }),
        with(agent("builder", "Builder", 3, "new"), |m| {
            m.model = Some("claude-opus-5".into())
        }),
    ];
    let shown = run(&switched, &PresentOptions { ..options });
    assert_eq!(
        shown.rows[0]
            .identity
            .as_ref()
            .unwrap()
            .author
            .maker
            .as_deref(),
        Some("openai")
    );
    assert_eq!(
        shown.rows[1]
            .identity
            .as_ref()
            .unwrap()
            .author
            .maker
            .as_deref(),
        Some("anthropic")
    );
}

#[test]
fn chat_avatars_stack_three_then_count_the_rest() {
    use zork_client_types::chat::ChatAgent;
    let agent = |id: &str, model: Option<&str>| ChatAgent {
        id: id.into(),
        name: Some(id.to_uppercase()),
        model: model.map(str::to_owned),
    };
    // No Agents (or an older Station): empty, the platform draws its plain mark.
    let none = chat_avatar(&[], 0);
    assert!(none.agents.is_empty());
    assert_eq!(none.more, 0);
    // One Agent shows its own avatar.
    let one = chat_avatar(&[agent("planner", Some("gpt-6-astra"))], 1);
    assert_eq!(one.agents.len(), 1);
    assert_eq!(one.agents[0].maker.as_deref(), Some("openai"));
    assert_eq!(one.agents[0].tint, preferred_slot("planner"));
    assert_eq!(one.more, 0);
    // Three fit; tints follow first appearance and never collide.
    let three = chat_avatar(
        &[
            agent("planner", Some("gpt-6-astra")),
            agent("builder", Some("deepseek-flash")),
            agent("review", None),
        ],
        3,
    );
    let tints: Vec<usize> = three.agents.iter().map(|a| a.tint).collect();
    assert_eq!(tints, [4, 1, 3]);
    assert_eq!(three.agents[2].maker, None);
    assert_eq!(three.agents[2].initial, "R");
    assert_eq!(three.more, 0);
    // Five: the Station lists the first four, the count covers all.
    let five = chat_avatar(
        &[
            agent("planner", None),
            agent("builder", None),
            agent("review", None),
            agent("tester", Some("house-model")),
        ],
        5,
    );
    assert_eq!(five.agents.len(), 3);
    assert_eq!(five.more, 2);
    let ids: Vec<&str> = five.agents.iter().map(|a| a.agent_id.as_str()).collect();
    assert_eq!(ids, ["planner", "builder", "review"]);
    // A summary without a count still counts what it lists.
    assert_eq!(chat_avatar(&five_agents(), 0).more, 1);
}

fn five_agents() -> Vec<zork_client_types::chat::ChatAgent> {
    ["a", "b", "c", "d"]
        .iter()
        .map(|id| zork_client_types::chat::ChatAgent {
            id: (*id).into(),
            name: None,
            model: None,
        })
        .collect()
}
