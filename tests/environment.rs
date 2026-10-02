use pctl::environment::parse;

#[test]
fn parses_exports_comments_and_quotes() {
    let source = "\
# leading comment

export PLAIN=value
SPACED = padded value   
INLINE=before # trailing comment
HASH=no#comment
DOUBLE=\"keeps # hash\" # comment
SINGLE='quoted value'
EMPTY=
EXPORTED_NAME=1
exportish=still-a-name
LENIENT=\"a\"b\"
";
    let environment = parse(source).unwrap();
    let expected = [
        ("PLAIN", "value"),
        ("SPACED", "padded value"),
        ("INLINE", "before"),
        ("HASH", "no#comment"),
        ("DOUBLE", "keeps # hash"),
        ("SINGLE", "quoted value"),
        ("EMPTY", ""),
        ("EXPORTED_NAME", "1"),
        ("exportish", "still-a-name"),
        ("LENIENT", "a\"b"),
    ];
    assert_eq!(environment.len(), expected.len());
    for (name, value) in expected {
        assert_eq!(
            environment.get(name).map(String::as_str),
            Some(value),
            "{name}"
        );
    }
}

#[test]
fn diagnostics_name_the_line_and_never_the_value() {
    for (source, expected) in [
        ("A=1\nnot an assignment", "line 2"),
        ("1BAD=x", "Invalid environment name on line 1"),
        ("export 1BAD=x", "Invalid environment name on line 1"),
        ("TOKEN=\"hunter2", "Unclosed quote on line 1"),
        ("TOKEN='hunter2", "Unclosed quote on line 1"),
        ("A=1\nA=2", "Duplicate environment name A"),
    ] {
        let error = parse(source).unwrap_err().to_string();
        assert!(error.contains(expected), "{source:?}: {error}");
        assert!(!error.contains("hunter2"), "{error}");
    }
}
