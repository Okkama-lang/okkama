//! Native ABI calls reach their providers through serialized SPB, checked
//! against the ABI table before they run.
#[path = "sparrow_support/mod.rs"]
mod support;
use okkama_compiler::{SourceFile, codegen, ir, lowering, syntax};

fn lower(body: &str) -> ir::Module {
    let source = SourceFile::new(
        "native.ok",
        format!(
            r#"module runtime/utf v1.0.0;
        native utf8_encode(origin is string) is @byte;
        native utf8_decode(origin is @byte) is string;
        native utf8_valid_prefix(origin is @byte) is int;
        native utf16_encode(origin is string, order is string) is @byte;
        native utf16_decode(origin is @byte, order is string) is string;
        native utf16_valid_prefix(origin is @byte, order is string) is int;
        native utf32_encode(origin is string, order is string) is @byte;
        native utf32_decode(origin is @byte, order is string) is string;
        native utf32_valid_prefix(origin is @byte, order is string) is int;
        main() is int {{ {body} }}"#
        ),
    )
    .unwrap();
    lowering::lower_program(&syntax::parse(&source, &syntax::ParseOptions::default()).unwrap())
        .unwrap()
}

/// `main` with `body` returns `expected` on Sparrow.
fn expect(body: &str, expected: &str) {
    let module = lower(body);

    support::assert_value(&module, expected);
}

#[test]
fn native_identity_and_signature_are_checked_before_execution() {
    let mut module = lower(r#"utf8_encode("x").size();"#);
    let function = module
        .functions
        .iter_mut()
        .find(|f| f.extern_symbol() == Some("runtime/utf@1::utf8_encode/1"))
        .unwrap();
    let attr = function
        .attrs
        .iter_mut()
        .find(|a| matches!(a, ir::Attr::Extern(_)))
        .unwrap();
    *attr = ir::Attr::Extern("runtime/utf@2::utf8_encode/1".into());

    // неизвестный provider — ошибка запуска, а не генерации.
    assert!(!matches!(support::sparrow(&module), support::Ending::Value(_)));

    // A declaration unlike the table's native of its symbol is refused by lowering, at the declaration (#1098).
    let source = SourceFile::new("bad-native.ok", "module runtime/utf v1.0.0; native utf8_encode(text is int) is int; main() is int { utf8_encode(1); }").unwrap();
    let refused =
        lowering::lower_program(&syntax::parse(&source, &syntax::ParseOptions::default()).unwrap());
    assert!(
        matches!(&refused, Err(error) if error.to_string().contains("does not match runtime/utf@1::utf8_encode/1 of the runtime")),
        "{refused:?}"
    );

    // SPB still checks a native against the ABI table when it verifies the program (#16): OIR whose extern
    // names another native of the table, with another signature, does not generate.
    let mut module = lower(r#"utf8_encode("x").size();"#);
    let function = module
        .functions
        .iter_mut()
        .find(|f| f.extern_symbol() == Some("runtime/utf@1::utf8_encode/1"))
        .unwrap();
    let attr = function.attrs.iter_mut().find(|a| matches!(a, ir::Attr::Extern(_))).unwrap();
    *attr = ir::Attr::Extern("runtime/utf@1::utf8_valid_prefix/1".into());
    assert!(
        matches!(codegen::sparrow::generate(&module), Err(error) if error.message.contains("does not match the signature"))
    );
    let module = lower(r#"utf16_encode("x", "middle").size();"#);
    assert!(!matches!(support::sparrow(&module), support::Ending::Value(_)));

}

#[test]
fn utf_native_calls_preserve_scalars_and_byte_order() {
    expect(r#"utf8_encode("Aé🙂").size();"#, "7");
    for encoding in ["utf8", "utf16", "utf32"] {
        for order in ["big", "little"] {
            let order = if encoding == "utf8" {
                String::new()
            } else {
                format!(", \"{order}\"")
            };
            expect(
                &format!(
                    "let text = \"Aé🙂\"; let bytes = {encoding}_encode(text{order}); if {encoding}_decode(bytes{order}) == text {{ {encoding}_valid_prefix(bytes{order}) - bytes.size() + 42; }} else {{ 0; }}"
                ),
                "42",
            );
        }
    }
    expect(
        r#"let bs = utf16_encode("A", "big"); bs[0].int() + bs[1].int();"#,
        "65",
    );
    expect(
        r#"let bs = utf32_encode("A", "little"); bs[0].int();"#,
        "65",
    );
    expect(r#"utf8_valid_prefix(@byte(65, 192, 128));"#, "1");
    expect(r#"utf16_valid_prefix(@byte(0, 65, 216, 0), "big");"#, "2");
    expect(
        r#"utf32_valid_prefix(@byte(0, 0, 0, 65, 0, 17, 0, 0), "big");"#,
        "4",
    );
}

#[test]
fn malformed_native_data_keeps_structured_failure_fields() {
    for (body, operation, encoding) in [
        (
            r#"utf8_decode(@byte(192, 128)).length();"#,
            "utf8_decode/1",
            "UTF-8",
        ),
        (
            r#"utf16_decode(@byte(216, 0), "big").length();"#,
            "utf16_decode/2",
            "UTF-16BE",
        ),
        (
            r#"utf32_decode(@byte(0, 0, 17, 0), "little").length();"#,
            "utf32_decode/2",
            "UTF-32LE",
        ),
    ] {
        let module = lower(body);
        let report = support::report(&module);
        assert!(report.contains("core.utf") && report.contains("invalid_data")
            && report.contains(&format!("runtime/utf@1::{operation}"))
            && report.contains(&format!("Bytes are not valid {encoding}")), "{report}");
    }
    expect(
        r#"if false { utf8_decode(@byte(255)).length(); } else { 42; }"#,
        "42",
    );
}

#[test]
fn native_failure_is_caught_as_concrete_type_or_error_role() {
    for body in [
        r#"try { utf8_decode(@byte(255)); 0; } catch e is NativeFailure { if e.domain() == "core.utf" && e.code() == "invalid_data" && e.operation() == "runtime/utf@1::utf8_decode/1" && e.message() == "Bytes are not valid UTF-8" { 42; } else { 1; } }"#,
        r#"try { utf8_decode(@byte(255)); 0; } catch e is Error { if e.text() == "Bytes are not valid UTF-8" { 42; } else { 1; } }"#,
        r#"try { utf8_decode(@byte(255)); 0; } catch e is NativeFailure { let err is Error = e; if err.text() == e.message() { 42; } else { 1; } }"#,
        r#"try { try { utf8_decode(@byte(255)); 0; } catch e is Error { throw e; 1; } } catch e is NativeFailure { 42; }"#,
        r#"try { try { utf8_decode(@byte(255)); 0; } catch e is NativeFailure { throw; 1; } } catch e is Error { 42; }"#,
    ] {

        expect(body, "42");
    }
}

#[test]
fn sparrow_native_failure_unwinds_cleanup() {
    for body in [
        r#"defer utf8_decode(@byte(255)); panic "primary";"#,
        r#"defer utf16_decode(@byte(0), "big"); utf8_decode(@byte(255)); 0;"#,
    ] {
        let module = lower(body);
        let report = support::report(&module);
        assert!(report.contains("Secondary cleanup failure:") && report.contains("invalid_data"), "{body}: {report}");
    }
}
