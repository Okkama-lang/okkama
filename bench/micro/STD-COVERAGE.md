# std micro-benchmark targets

356 of 435 non-runtime module files have an explicitly declared measured action.

This is not API, branch or test coverage. A benchmark targeting one action does not cover the entire module. Harness imports do not count. Builtin string operations are listed in manifest.json separately.

| module | measured by |
|---|---|
| `archive/entry` | tar-round-trip |
| `archive/limits` | tar-round-trip |
| `archive/metadata` | tar-round-trip |
| `archive/path` | archive-path, tar-round-trip |
| `archive/stream` | — |
| `archive/tar` | tar-round-trip |
| `archive/zip` | zip-round-trip |
| `async/atomic` | defer-observed, destroy-observed, http1-body-writing-output, http1-message-writing-output, http2-output-submit, websocket-output-message, http2-preface-settings, http-proxy-tunnel-ready, imap-exchange-perform, http2-input-run |
| `async/barrier` | async-barrier |
| `async/cancellation` | http-request-context-parameters, async-cancellation |
| `async/channel` | async-channel-items |
| `async/collect` | async-all-futures |
| `async/event` | async-event |
| `async/lock` | async-lock-guard |
| `async/scope` | async-future-group |
| `async/semaphore` | async-semaphore |
| `async/timeout` | async-within |
| `calendar` | calendar-month |
| `cell` | cell-replace |
| `checksum/adler32` | adler32 |
| `checksum/crc` | crc32 |
| `checksum/state` | — |
| `cli/arguments` | process-arguments |
| `cli/command` | cli-invocation |
| `collections/combinatorics` | combinations-count |
| `collections/combining` | zipped-chained |
| `collections/deque` | deque-ends |
| `collections/enumeration` | enumerated |
| `collections/frequencies` | frequencies-count |
| `collections/heap` | heap-top |
| `collections/iteration` | regex-parts |
| `collections/list` | heap-top |
| `collections/ordering` | sorted |
| `collections/repetition` | repeated |
| `collections/sequence` | http-static-indexes, imap-metadata-uids, http-cookie-jar-store |
| `collections/set` | keyed-set, ordered-set-union |
| `collections/views` | views-filtered-chunked |
| `compare` | — |
| `compression/brotli` | brotli-round-trip |
| `compression/deflate` | deflate-round-trip |
| `compression/gzip` | gzip-round-trip, http-body-coding-contained |
| `compression/internal/codec` | native-codec |
| `compression/read` | compressed-reading |
| `compression/state` | compressed-reading |
| `compression/zlib` | zlib-round-trip |
| `compression/zstd` | zstd-round-trip |
| `config/document` | layered-settings |
| `config/environment` | layered-settings |
| `config/ini` | ini-parse |
| `config/key` | setting-key |
| `config/source` | layered-settings |
| `config/value` | layered-settings |
| `crypto/aead` | — |
| `crypto/aes_gcm` | aes-gcm-round-trip |
| `crypto/chacha20_poly1305` | chacha20-round-trip |
| `crypto/constant_time` | constant-time-equal |
| `crypto/ecdsa` | ecdsa-sign-verify |
| `crypto/ed25519` | ed25519-sign-verify |
| `crypto/error` | decimal-argument |
| `crypto/hmac` | hmac-sha256 |
| `crypto/jwt` | jwt-hs256-sign |
| `crypto/kdf` | hkdf-sha256, pbkdf2-sha256 |
| `crypto/key` | ed25519-sign-verify |
| `crypto/key_format` | parsed-key-algorithm |
| `crypto/nonce` | counter-nonce |
| `crypto/password` | argon2-password |
| `crypto/rsa` | rsa-pkcs1-sign |
| `crypto/secret` | hkdf-sha256, pbkdf2-sha256, x25519-public-key, http-auth-authorization, smtp-session-response, http-digest-response-digest |
| `crypto/x25519` | x25519-public-key |
| `crypto/x509/certificate` | x509-certificate |
| `crypto/x509/name` | x509-certificate |
| `crypto/x509/trust` | http-client-protocols-selected, verified-chain |
| `crypto/x509/value` | x509-certificate |
| `csv` | csv-round-trip |
| `csv/input` | csv-input |
| `csv/stream` | csv-stream |
| `db/acquisition` | — |
| `db/connection` | — |
| `db/error` | — |
| `db/pool` | — |
| `db/postgres` | — |
| `db/postgres/auth` | — |
| `db/postgres/wire` | — |
| `db/prepared` | — |
| `db/query` | — |
| `db/read` | — |
| `db/row` | — |
| `db/sqlite` | — |
| `db/sqlite/backup` | — |
| `db/sqlite/blob` | — |
| `db/sqlite/script` | — |
| `db/statement` | — |
| `db/transaction` | — |
| `db/value` | — |
| `debug/dumper` | dumper-shape |
| `diff` | diff-edits |
| `digest/blake2` | blake2b |
| `digest/blocks` | — |
| `digest/host` | host-digest |
| `digest/legacy` | md5, sha1 |
| `digest/read` | read-digest |
| `digest/sha2` | sha256, sha512 |
| `digest/sha3` | sha3-256 |
| `digest/shake` | shake128 |
| `digest/state` | — |
| `encoding/base32` | base32-encode |
| `encoding/base64` | base64-encode |
| `encoding/base85` | base85-encode, base85-decode |
| `encoding/binary` | binary-int-round-trip, binary-float-round-trip, byte-range-integer |
| `encoding/bytes` | byte-search, byte-range-integer |
| `encoding/hex` | hex-encode, hex-decode |
| `encoding/pem` | pem-encode, pem-parse |
| `encoding/quoted_printable` | qp-encode, qp-decode |
| `encoding/single_byte` | latin1-decode, ascii-encode |
| `encoding/utf` | utf8-encode, utf16-encode, form-parse, plist-parse, imap-response, pop3-size, pop3-reply, smtp-reply-fed, http1-reading-head, imap-reading-read, smtp-reading-read, pop3-reading-read, imap-message-uid, imap-state-messages, smtp-data-bytes, http1-body-bytes, http-proxy-tunnel-ready, imap-exchange-perform |
| `error` | — |
| `fs/directory` | fs-temporary-tree |
| `fs/entry` | fs-temporary-tree |
| `fs/error` | — |
| `fs/file` | fs-temporary-tree |
| `fs/glob` | fs-temporary-tree |
| `fs/link` | fs-temporary-tree |
| `fs/lock` | fs-temporary-tree |
| `fs/path` | mime-extension, unix-address, fs-path |
| `fs/root` | fs-temporary-tree |
| `fs/stat` | fs-temporary-tree |
| `fs/temp` | fs-temporary-tree |
| `fs/watch` | — |
| `html/entities` | html-escape, html-unescape |
| `html/events` | html-events |
| `http/auth` | http-auth-authorization |
| `http/auth_challenge` | http-auth-challenge-values, http-digest-response-digest |
| `http/body` | http-body-opening, http-message-new, http1-body-check-known, http1-reading-head, http2-reading-frame, websocket-reading-frame, websocket-upgrade-bytes, imap-reading-read, smtp-reading-read, pop3-reading-read, http-form-parameters, http-body-coding-contained, http1-body-writing-output, http1-message-writing-output, http1-body-bytes, http-conversation-response, http2-preface-settings, http-proxy-tunnel-ready, imap-exchange-perform, http2-input-run |
| `http/body_coding` | http-body-coding-contained |
| `http/cache` | http-cache-response |
| `http/cache_admission` | http-cache-admission-join |
| `http/cache_control` | http-cache-control-seconds |
| `http/cache_freshness` | http-cache-freshness-age, http-cache-validation-request |
| `http/cache_memory` | http-cache-memory-headers, http-cache-response, http-cache-validation-request |
| `http/cache_validation` | http-cache-validation-request |
| `http/client` | — |
| `http/client_policy` | http-client-policy-http1, http-client-protocols-selected |
| `http/client_protocols` | http-client-protocols-selected |
| `http/client_request` | http-client-request |
| `http/conditional` | http-entity-tags |
| `http/connection` | — |
| `http/content_coding` | http-content-coding-string |
| `http/conversation` | http-conversation-response |
| `http/cookie` | http-cookie-pair, web-session-text, http-cookie-jar-store |
| `http/cookie_client` | http-cookie-client-context |
| `http/cookie_date` | cookie-date |
| `http/cookie_jar` | http-cookie-jar-store |
| `http/cookie_policy` | http-cookie-policy-matches, http-cookie-client-context, http-cookie-jar-store |
| `http/date` | http-date-format, http-date-parse |
| `http/digest` | http-digest-response-digest |
| `http/endpoint` | http-endpoint-allowed, websocket-client-limits |
| `http/environment_proxy` | http-environment-proxy |
| `http/exchange` | http-exchange-string, web-testing-get, http-request-context-parameters, http-retry-response, http-redirect-response, http-cache-response, http-cache-validation-request |
| `http/form` | http-form-parameters |
| `http/handler` | http-handler-response |
| `http/header` | http-auth-authorization, http-cache-control-seconds, http-cache-memory-headers, http-header-int, http2-body-known, http-proxy-request, websocket-origin-allowed, web-reply-response, web-internal-reply-response, web-exception-response, http-cache-freshness-age, http-trailers-check, http-cookie-jar-store, http1-body-writing-output, http1-body-bytes, http-retry-response, http-redirect-response, http-cache-response, http-cache-validation-request |
| `http/http1_body` | http1-body-bytes |
| `http/http1_body_check` | http1-body-check-known |
| `http/http1_body_writing` | http1-body-writing-output |
| `http/http1_chunk` | http1-chunk-size |
| `http/http1_continue` | http1-continue-begun |
| `http/http1_framing` | http1-framing-plan, http1-body-check-known, http1-body-writing-output, http1-body-bytes |
| `http/http1_head` | http1-head, http1-framing-plan, http1-body-check-known, http1-writing-bytes |
| `http/http1_message_writing` | http1-message-writing-output |
| `http/http1_reading` | http1-reading-head |
| `http/http1_session` | http1-session-response |
| `http/http1_writing` | http1-writing-bytes |
| `http/http2_connection` | — |
| `http/http2_server` | — |
| `http/http2_session` | http2-session-response |
| `http/http2_settings` | http2-settings-bytes, http2-preface-settings, http2-input-run |
| `http/http2_tls` | http2-tls-version |
| `http/internal/hpack` | hpack-decoded, http2-headers-frames |
| `http/internal/hpack_primitives` | hpack-primitives-bytes |
| `http/internal/hpack_static` | hpack-static-names |
| `http/internal/hpack_tables` | hpack-tables-added, hpack-decoded, http2-fields, http2-headers-frames |
| `http/internal/http2_body` | http2-body-known |
| `http/internal/http2_controls` | http2-controls-frame |
| `http/internal/http2_fields` | http2-fields |
| `http/internal/http2_flow` | http2-flow-sent |
| `http/internal/http2_frame` | http2-frame-bytes, http2-controls-frame, http2-headers-frames, http2-reading-frame, http2-output-submit, http2-preface-settings, http2-input-run |
| `http/internal/http2_headers` | http2-headers-frames |
| `http/internal/http2_input` | http2-input-run |
| `http/internal/http2_life` | http2-life-allocated |
| `http/internal/http2_output` | http2-output-submit |
| `http/internal/http2_preface` | http2-preface-settings |
| `http/internal/http2_reading` | http2-reading-frame |
| `http/internal/http2_receiving` | http2-receiving-bodies |
| `http/internal/http2_sending` | http2-sending-close |
| `http/internal/http2_settings` | http2-settings-setting, http2-preface-settings, http2-input-run |
| `http/internal/trailers` | http-trailers-check |
| `http/internal/websocket_frame` | websocket-frame-bytes, websocket-reading-frame, websocket-messages-accept |
| `http/internal/websocket_messages` | websocket-messages-accept |
| `http/internal/websocket_output` | websocket-output-message |
| `http/internal/websocket_reading` | websocket-reading-frame |
| `http/internal/websocket_upgrade` | websocket-upgrade-bytes |
| `http/internal/wire_bytes` | http-wire-bytes |
| `http/media_type` | http-media-type |
| `http/message` | http-client-request, http-message-new, websocket-origin-allowed, http-cookie-client-context, http-handler-response, http2-fields, http1-message-writing-output, http1-session-response, http2-session-response, http-conversation-response, http-retry-response, http-redirect-response, http-cache-response, http-cache-validation-request |
| `http/method` | http-method-semantics |
| `http/negotiation` | http-quality |
| `http/proxy` | — |
| `http/proxy_connection` | — |
| `http/proxy_policy` | http-proxy-policy-url, http-environment-proxy |
| `http/proxy_request` | http-proxy-request |
| `http/proxy_tunnel` | http-proxy-tunnel-ready |
| `http/redirect` | http-redirect-response |
| `http/request_context` | http-request-context-parameters |
| `http/retry` | http-retry-response |
| `http/robots` | http-robots-allowed |
| `http/routing` | http-routing-match, web-routing-routes |
| `http/selected_connection` | — |
| `http/server` | — |
| `http/server_policy` | http-server-policy-handling-timeout |
| `http/sse` | http-sse-retry |
| `http/static` | http-static-indexes |
| `http/status` | http-status-phrase, http-handler-response, http1-session-response, http2-session-response, http-conversation-response |
| `http/target` | http-request-target, http-client-request, http1-head |
| `http/tls_session` | http-tls-session-version |
| `http/token` | http-token-string |
| `http/transport_observation` | http-transport-observation-begin |
| `http/version` | http-version |
| `http/websocket` | websocket-text, websocket-server-protocols, websocket-client-limits, websocket-messages-accept, websocket-output-message |
| `http/websocket_client` | websocket-client-limits |
| `http/websocket_handshake` | websocket-handshake-string |
| `http/websocket_origin` | websocket-origin-allowed, websocket-server-protocols |
| `http/websocket_server` | websocket-server-protocols |
| `http/websocket_session` | — |
| `http/xmlrpc` | http-xmlrpc-bytes |
| `i18n/catalog` | message-catalog |
| `i18n/gettext` | gettext-catalog |
| `i18n/operands` | — |
| `i18n/plural` | plural-ru |
| `io/buffer` | template-stream, read-until-end, csv-input, read-digest, zip-round-trip, json-stream-logs |
| `io/decorators` | mime-transfer |
| `io/input` | byte-input-lines |
| `io/stdin` | — |
| `io/stdout` | — |
| `io/stream` | http1-body-writing-output, http1-message-writing-output, http2-output-submit, websocket-output-message, http2-preface-settings, http-proxy-tunnel-ready, imap-exchange-perform |
| `io/text` | template-stream |
| `io/transfer` | read-until-end |
| `json` | json-parse, json-round-trip, json-render, template-render, html-template, template-json-value, template-sections, jwt-hs256-sign, json-log-line |
| `locale/case` | locale-case |
| `locale/collation` | — |
| `locale/currency` | currency-digits |
| `locale/data` | locale-fallbacks |
| `locale/date_time` | localized-date |
| `locale/internal/currencies` | — |
| `locale/internal/day_periods` | — |
| `locale/internal/gregorian` | — |
| `locale/internal/iana` | — |
| `locale/internal/likely` | — |
| `locale/internal/lists` | — |
| `locale/internal/numbers` | — |
| `locale/internal/parents` | — |
| `locale/internal/plurals` | — |
| `locale/internal/zone_meta` | — |
| `locale/internal/zone_names` | — |
| `locale/likely` | likely-locale |
| `locale/list` | locale-list |
| `locale/match` | basic-language-range |
| `locale/number` | locale-decimal |
| `locale/system` | language-preferences |
| `locale/tag` | locale-case, plural-ru, locale-decimal, locale-list |
| `logging/enrichment` | log-pipeline |
| `logging/field` | json-log-line |
| `logging/file` | — |
| `logging/filter` | log-pipeline |
| `logging/level` | logger-levels |
| `logging/line` | json-log-line |
| `logging/logger` | logger-levels |
| `logging/name` | json-log-line |
| `logging/named` | log-pipeline |
| `logging/queue` | — |
| `logging/record` | json-log-line |
| `logging/redact` | log-pipeline |
| `logging/rotation` | size-rotation |
| `logging/stream` | json-stream-logs |
| `logging/syslog` | — |
| `logging/write` | log-pipeline, logger-levels |
| `mail/imap` | imap-name |
| `mail/imap_command` | imap-command-bytes, imap-exchange-perform |
| `mail/imap_command_reading` | imap-command-bytes, imap-command-reading-string, imap-state-messages, imap-exchange-perform |
| `mail/imap_data` | imap-data-tokens |
| `mail/imap_exchange` | imap-exchange-perform |
| `mail/imap_message` | imap-message-uid |
| `mail/imap_metadata` | imap-metadata-uids |
| `mail/imap_name` | imap-command-bytes, imap-name-wire-name, imap-session-expire, imap-exchange-perform |
| `mail/imap_reading` | imap-reading-read |
| `mail/imap_response` | imap-response, imap-message-uid, imap-state-messages |
| `mail/imap_search` | imap-search-key |
| `mail/imap_session` | imap-session-expire |
| `mail/imap_state` | imap-state-messages |
| `mail/imap_uid` | imap-metadata-uids, imap-uid-argument, imap-message-uid |
| `mail/mailbox` | mail-mailbox-membership |
| `mail/mbox` | mbox-string |
| `mail/mbox_store` | — |
| `mail/message` | mail-message-string, smtp-data-bytes |
| `mail/pop3` | pop3-size, pop3-session-bytes |
| `mail/pop3_reading` | pop3-reading-read |
| `mail/pop3_reply` | pop3-size, pop3-reply |
| `mail/pop3_session` | pop3-session-bytes |
| `mail/smtp` | smtp-maximum-size, smtp-data-bytes |
| `mail/smtp_data` | smtp-data-bytes |
| `mail/smtp_reading` | smtp-reading-read |
| `mail/smtp_reply` | smtp-maximum-size, smtp-reply-fed, smtp-data-bytes |
| `mail/smtp_session` | smtp-session-response |
| `math/aggregate` | aggregate-extremes |
| `math/constants` | — |
| `math/decimal` | locale-decimal, decimal-plus, shortest-float |
| `math/rounding` | — |
| `message/header` | message-header |
| `mime/disposition` | mime-disposition |
| `mime/media_type` | mime-media-type, mime-extension |
| `mime/part` | mime-text-body |
| `mime/transfer` | mime-transfer |
| `mime/types` | mime-extension |
| `mime/word` | mime-encoded-word |
| `net/dns` | dns-fixed-record |
| `net/endpoint` | dns-fixed-record, host-endpoint, public-suffix, http-endpoint-allowed, http-request-context-parameters, http-retry-response, http-redirect-response, http-cache-response, http-cache-validation-request |
| `net/failure` | network-failure |
| `net/interface` | network-interface |
| `net/ip` | ip-parse, http-endpoint-allowed, http-request-context-parameters, http-retry-response, http-redirect-response, http-cache-response, http-cache-validation-request |
| `net/ip_registry` | ip-classification |
| `net/multicast` | multicast-options |
| `net/public_suffix` | public-suffix |
| `net/stream` | connection-endpoints, http-request-context-parameters, http-retry-response, http-redirect-response, http-cache-response, http-cache-validation-request |
| `net/tcp` | tcp-options |
| `net/tcp_racing` | — |
| `net/tls` | tls-details, http-client-protocols-selected |
| `net/udp` | udp-datagram |
| `net/unix` | unix-address |
| `numbers/bigint` | bigint-add, operator-adding |
| `numbers/bigrational` | bigrational-normalize |
| `numbers/float` | shortest-float |
| `operators` | operator-adding |
| `os/account` | — |
| `os/command` | command-string |
| `os/command_capture` | — |
| `os/directory` | — |
| `os/environment` | http-environment-proxy, environment-values, fs-temporary-tree |
| `os/pipeline_capture` | — |
| `os/platform` | — |
| `os/process` | process-exit |
| `os/run` | — |
| `os/signal` | — |
| `os/storage` | — |
| `parallel/map` | parallel-map |
| `plist/parse` | plist-parse |
| `plist/value` | plist-parse, plist-integer, plist-write |
| `plist/write` | plist-write |
| `random/distribution` | seeded-uniform |
| `random/source` | pcg-bytes, seeded-uniform, websocket-output-message |
| `regex/escape` | regex-escape |
| `regex/match` | regex-capture |
| `regex/pattern` | regex-first-match, regex-replace |
| `regex/range` | regex-range |
| `regex/replace` | regex-replace |
| `regex/search` | regex-first-match, regex-capture |
| `regex/split` | regex-parts |
| `size` | sizing-role |
| `statistics` | statistics-median |
| `strings/buffer` | text-buffer |
| `strings/format` | formatted |
| `strings/join` | joined |
| `strings/parse` | int-parse, hex-number-parse |
| `template/compile` | text-template, text-template-library |
| `template/engine` | template-render |
| `template/html` | html-template, html-template-library |
| `template/html_library` | html-template-library |
| `template/json` | template-json-value |
| `template/library` | text-template-library |
| `template/model` | template-binding, template-stream |
| `template/path` | template-path |
| `template/render` | template-stream |
| `template/sections` | template-sections |
| `template/tree` | template-tree |
| `terminal/capability` | terminal-size |
| `terminal/error` | terminal-failure |
| `terminal/mode` | — |
| `terminal/prompt` | — |
| `terminal/screen` | — |
| `terminal/session` | — |
| `terminal/style` | terminal-color |
| `terminal/width` | display-width |
| `testing/check` | — |
| `testing/context` | — |
| `testing/double` | — |
| `testing/error` | error-matching |
| `testing/generation` | integer-generation |
| `testing/property` | — |
| `testing/report` | — |
| `testing/result` | test-report-counts |
| `testing/runner` | — |
| `testing/signal` | — |
| `testing/snapshot` | text-snapshot |
| `testing/time` | — |
| `text/layout` | text-indent |
| `time/clock` | instant-shift, rfc3339-parse, http-cache-freshness-age, http-cache-validation-request |
| `time/date` | date-shift, date-format, iso-date, imap-search-key |
| `time/day` | time-of-day |
| `time/duration` | instant-shift, duration-add, local-datetime-shift, rfc3339-parse, fixed-zone, http-server-policy-handling-timeout, http-sse-retry, http-cache-freshness-age, websocket-server-protocols, websocket-client-limits, http1-continue-begun, http-cache-validation-request |
| `time/format` | date-format |
| `time/iso` | iso-text |
| `time/local_datetime` | local-datetime-shift |
| `time/rfc3339` | rfc3339-parse |
| `time/zone` | fixed-zone |
| `toml/parse` | toml-parse |
| `toml/value` | toml-parse |
| `unicode/character` | unicode-category |
| `unicode/normalization` | normalize-nfc |
| `url/form` | form-parse |
| `url/percent` | percent-encode, percent-decode |
| `url/query` | query-parse, form-parse, http-form-parameters |
| `url/value` | url-parts, http-exchange-string, http-proxy-policy-url, http-proxy-request, http-robots-allowed, websocket-origin-allowed, http-cookie-client-context, http-environment-proxy, http-endpoint-allowed, http-cookie-jar-store, http-proxy-tunnel-ready |
| `uuid/generation` | uuid5-name |
| `uuid/value` | uuid-parse |
| `web/app` | — |
| `web/around` | web-around-routes |
| `web/exception` | web-exception-response |
| `web/internal/reply` | web-internal-reply-response |
| `web/reply` | web-reply-response, web-testing-get |
| `web/request` | web-request-link |
| `web/routing` | web-routing-routes, web-testing-get |
| `web/session` | web-session-text |
| `web/testing` | web-testing-get |
| `xml/name` | xml-name |
| `xml/node` | xml-attribute |
| `xml/parse` | xml-attribute |
| `xml/write` | xml-escape |
