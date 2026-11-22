# std micro-benchmark targets

399 of 436 non-runtime module files have an explicitly declared measured action.

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
| `async/scope` | async-future-group, queued-logs |
| `async/semaphore` | async-semaphore |
| `async/timeout` | async-within |
| `calendar` | calendar-month |
| `cell` | cell-replace |
| `checksum/adler32` | adler32 |
| `checksum/crc` | crc32 |
| `checksum/state` | — |
| `cli/arguments` | process-arguments |
| `cli/command` | cli-invocation |
| `collections/combinatorics` | combinations-count, collections-combinatorics |
| `collections/combining` | zipped-chained, collections-views-reversed |
| `collections/deque` | deque-ends, collections-heap-deque |
| `collections/enumeration` | enumerated |
| `collections/frequencies` | frequencies-count, collections-frequencies |
| `collections/heap` | heap-top, collections-heap-deque |
| `collections/iteration` | regex-parts |
| `collections/list` | heap-top |
| `collections/ordering` | sorted, collections-sorted, collections-ordering-search |
| `collections/repetition` | repeated, collections-views-reversed |
| `collections/sequence` | http-static-indexes, imap-metadata-uids, http-cookie-jar-store |
| `collections/set` | keyed-set, ordered-set-union, collections-set-union, collections-set-difference, collections-set-edits |
| `collections/views` | views-filtered-chunked, collections-views-bounds, collections-views-transform, collections-views-reversed |
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
| `crypto/aes_gcm` | aes-gcm-round-trip, crypto-aead-text |
| `crypto/chacha20_poly1305` | chacha20-round-trip, crypto-aead-text, crypto-xchacha |
| `crypto/constant_time` | constant-time-equal |
| `crypto/ecdsa` | ecdsa-sign-verify, crypto-ecdsa-keys |
| `crypto/ed25519` | ed25519-sign-verify, crypto-ed25519-x25519 |
| `crypto/error` | decimal-argument, crypto-host-nonce |
| `crypto/hmac` | hmac-sha256, crypto-hmac-secret |
| `crypto/jwt` | jwt-hs256-sign, crypto-ecdsa-keys, crypto-jwt-keys, crypto-rsa-keys, crypto-rsa-pss-oaep |
| `crypto/kdf` | hkdf-sha256, pbkdf2-sha256 |
| `crypto/key` | ed25519-sign-verify |
| `crypto/key_format` | parsed-key-algorithm, crypto-key-format |
| `crypto/nonce` | counter-nonce, crypto-host-nonce |
| `crypto/password` | argon2-password, crypto-hmac-secret, crypto-host-nonce |
| `crypto/rsa` | rsa-pkcs1-sign, crypto-rsa-keys, crypto-rsa-pss-oaep |
| `crypto/secret` | hkdf-sha256, pbkdf2-sha256, x25519-public-key, http-auth-authorization, smtp-session-response, http-digest-response-digest, crypto-hmac-secret |
| `crypto/x25519` | x25519-public-key, crypto-ed25519-x25519 |
| `crypto/x509/certificate` | x509-certificate, crypto-x509-values, crypto-x509-extensions, crypto-x509-chain |
| `crypto/x509/name` | x509-certificate, crypto-x509-values |
| `crypto/x509/trust` | http-client-protocols-selected, verified-chain, crypto-x509-chain |
| `crypto/x509/value` | x509-certificate, crypto-x509-values |
| `csv` | csv-round-trip |
| `csv/input` | csv-input |
| `csv/stream` | csv-stream |
| `db/acquisition` | sqlite-file-pool, sqlite-acquisition |
| `db/connection` | sqlite-file-pool |
| `db/error` | — |
| `db/pool` | sqlite-file-pool, db-pool-state-lease, db-pool-state-retire, db-pooled-connection, db-pooled-transaction, db-pool-prepared, db-physical |
| `db/postgres` | — |
| `db/postgres/auth` | — |
| `db/postgres/wire` | — |
| `db/prepared` | sqlite-prepared-blob, sqlite-session-prepare, sqlite-prepared-transaction |
| `db/query` | sqlite-prepared-blob, db-query-forms |
| `db/read` | db-column-reader, db-column-reader-typed, db-page-reader, db-maybe-reader, db-maybe-presence, db-presence-reader |
| `db/row` | sqlite-memory-aggregate, db-column-reader, db-page-reader, db-maybe-presence, db-presence-reader, db-statement-values |
| `db/sqlite` | sqlite-memory-aggregate, sqlite-prepared-blob, sqlite-file-pool, sqlite-session-execute, sqlite-session-rows, sqlite-session-prepare, sqlite-transaction-control, sqlite-transaction-work, sqlite-blob-session, sqlite-script, sqlite-backup, sqlite-failures, sqlite-prepared-transaction, sqlite-acquisition |
| `db/sqlite/backup` | sqlite-file-pool, sqlite-backup |
| `db/sqlite/blob` | sqlite-prepared-blob, sqlite-blob-session |
| `db/sqlite/script` | sqlite-prepared-blob, sqlite-script |
| `db/statement` | sqlite-memory-aggregate, db-statement-values |
| `db/transaction` | sqlite-memory-aggregate |
| `db/value` | sqlite-memory-aggregate, sqlite-prepared-blob, db-column-reader, db-value-capture-scalars, db-value-capture-text, db-value-capture-protocol, db-value-print, db-value-kinds, db-value-readers, db-value-native |
| `debug/dumper` | dumper-shape |
| `diff` | diff-edits |
| `digest/blake2` | blake2b, digest-blake2b, digest-blake2s |
| `digest/blocks` | digest-legacy-blocks |
| `digest/host` | host-digest |
| `digest/legacy` | md5, sha1, digest-legacy-blocks |
| `digest/read` | read-digest, digest-read |
| `digest/sha2` | sha256, sha512, digest-sha2-small, digest-sha2-wide |
| `digest/sha3` | sha3-256, digest-sha3-small, digest-sha3-wide |
| `digest/shake` | shake128, digest-shake |
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
| `fs/directory` | fs-temporary-tree, fs-entries, fs-tree-links |
| `fs/entry` | fs-temporary-tree, fs-entries |
| `fs/error` | fs-root-stat |
| `fs/file` | fs-temporary-tree, fs-file-bytes, fs-file-text, fs-temp-finish |
| `fs/glob` | fs-temporary-tree |
| `fs/link` | fs-temporary-tree, fs-tree-links |
| `fs/lock` | fs-temporary-tree, fs-tree-links |
| `fs/path` | mime-extension, unix-address, fs-path, fs-path-forms |
| `fs/root` | fs-temporary-tree, fs-root-stat |
| `fs/stat` | fs-temporary-tree, fs-stat-values, fs-root-stat |
| `fs/temp` | fs-temporary-tree, fs-temp-file, fs-temp-finish |
| `fs/watch` | — |
| `html/entities` | html-escape, html-unescape |
| `html/events` | html-events |
| `http/auth` | http-auth-authorization, http-auth-scoped |
| `http/auth_challenge` | http-auth-challenge-values, http-digest-response-digest, http-auth-challenge-fields |
| `http/body` | http-body-opening, http-message-new, http1-body-check-known, http1-reading-head, http2-reading-frame, websocket-reading-frame, websocket-upgrade-bytes, imap-reading-read, smtp-reading-read, pop3-reading-read, http-form-parameters, http-body-coding-contained, http1-body-writing-output, http1-message-writing-output, http1-body-bytes, http-conversation-response, http2-preface-settings, http-proxy-tunnel-ready, imap-exchange-perform, http2-input-run, http-body-stream, http-body-content |
| `http/body_coding` | http-body-coding-contained, http-body-gzip, http-body-brotli, http-body-brotli-contained |
| `http/cache` | http-cache-response |
| `http/cache_admission` | http-cache-admission-join, http-cache-admissions |
| `http/cache_control` | http-cache-control-seconds, http-cache-directives |
| `http/cache_freshness` | http-cache-freshness-age, http-cache-validation-request, http-cache-entry |
| `http/cache_memory` | http-cache-memory-headers, http-cache-response, http-cache-validation-request, http-cache-entry, http-cache-storage |
| `http/cache_validation` | http-cache-validation-request, http-cache-revalidate |
| `http/client` | — |
| `http/client_policy` | http-client-policy-http1, http-client-protocols-selected, http-policy-protocols |
| `http/client_protocols` | http-client-protocols-selected, http-policy-protocols |
| `http/client_request` | http-client-request |
| `http/conditional` | http-entity-tags, http-conditional-tags, http-conditional-ranges |
| `http/connection` | http1-direct-exchange, http-connection-policies, http-acquisition-cleanup |
| `http/content_coding` | http-content-coding-string |
| `http/conversation` | http-conversation-response, http1-session-memory, http1-direct-exchange, http2-conversation-exchange |
| `http/cookie` | http-cookie-pair, web-session-text, http-cookie-jar-store, http-cookie-render, http-cookie-read |
| `http/cookie_client` | http-cookie-client-context, http-exchange-decorators |
| `http/cookie_date` | cookie-date |
| `http/cookie_jar` | http-cookie-jar-store, http-cookie-storage, http-cookie-files |
| `http/cookie_policy` | http-cookie-policy-matches, http-cookie-client-context, http-cookie-jar-store, http-cookie-storage |
| `http/date` | http-date-format, http-date-parse, http-cache-directives |
| `http/digest` | http-digest-response-digest, http-digest-fields, http-digest-exchange |
| `http/endpoint` | http-endpoint-allowed, websocket-client-limits, http-endpoint-selection |
| `http/environment_proxy` | http-environment-proxy |
| `http/exchange` | http-exchange-string, web-testing-get, http-request-context-parameters, http-retry-response, http-redirect-response, http-cache-response, http-cache-validation-request, http-exchange-body |
| `http/form` | http-form-parameters, http-form-read |
| `http/handler` | http-handler-response |
| `http/header` | http-auth-authorization, http-cache-control-seconds, http-cache-memory-headers, http-header-int, http2-body-known, http-proxy-request, websocket-origin-allowed, web-reply-response, web-internal-reply-response, web-exception-response, http-cache-freshness-age, http-trailers-check, http-cookie-jar-store, http1-body-writing-output, http1-body-bytes, http-retry-response, http-redirect-response, http-cache-response, http-cache-validation-request, http-header-queries, http-header-transforms |
| `http/http1_body` | http1-body-bytes, http1-body-remainder |
| `http/http1_body_check` | http1-body-check-known |
| `http/http1_body_writing` | http1-body-writing-output |
| `http/http1_chunk` | http1-chunk-size, http1-trailer-parse |
| `http/http1_continue` | http1-continue-begun, http1-request-progress |
| `http/http1_framing` | http1-framing-plan, http1-body-check-known, http1-body-writing-output, http1-body-bytes, http1-framing-fields |
| `http/http1_head` | http1-head, http1-framing-plan, http1-body-check-known, http1-writing-bytes, http1-framing-fields, http1-head-objects, http1-request-progress |
| `http/http1_message_writing` | http1-message-writing-output, http1-wire-plans, http1-request-progress |
| `http/http1_reading` | http1-reading-head, http1-request-progress |
| `http/http1_session` | http1-session-response, http1-session-memory, http1-direct-exchange |
| `http/http1_writing` | http1-writing-bytes, http1-wire-plans |
| `http/http2_connection` | http-connection-policies |
| `http/http2_server` | http2-server-lifecycle |
| `http/http2_session` | http2-session-response, http2-session-exchange |
| `http/http2_settings` | http2-settings-bytes, http2-preface-settings, http2-input-run |
| `http/http2_tls` | http2-tls-version |
| `http/internal/hpack` | hpack-decoded, http2-headers-frames, http2-header-blocks |
| `http/internal/hpack_primitives` | hpack-primitives-bytes, hpack-primitives-parse |
| `http/internal/hpack_static` | hpack-static-names |
| `http/internal/hpack_tables` | hpack-tables-added, hpack-decoded, http2-fields, http2-headers-frames, hpack-table-lookup |
| `http/internal/http2_body` | http2-body-known, http2-body-reading, http2-body-failure |
| `http/internal/http2_controls` | http2-controls-frame, http2-control-frames |
| `http/internal/http2_fields` | http2-fields, http2-field-sections |
| `http/internal/http2_flow` | http2-flow-sent, http2-flow-windows, http2-flow-payloads |
| `http/internal/http2_frame` | http2-frame-bytes, http2-controls-frame, http2-headers-frames, http2-reading-frame, http2-output-submit, http2-preface-settings, http2-input-run, http2-frame-validation |
| `http/internal/http2_headers` | http2-headers-frames, http2-header-blocks |
| `http/internal/http2_input` | http2-input-run |
| `http/internal/http2_life` | http2-life-allocated, http2-stream-life |
| `http/internal/http2_output` | http2-output-submit, http2-output-delivery, http2-output-release, http2-header-blocks |
| `http/internal/http2_preface` | http2-preface-settings, http2-server-preface |
| `http/internal/http2_reading` | http2-reading-frame |
| `http/internal/http2_receiving` | http2-receiving-bodies, http2-receive-ledger |
| `http/internal/http2_sending` | http2-sending-close, http2-send-reservations, http2-send-data |
| `http/internal/http2_settings` | http2-settings-setting, http2-preface-settings, http2-input-run, http2-server-preface |
| `http/internal/trailers` | http-trailers-check |
| `http/internal/websocket_frame` | websocket-frame-bytes, websocket-reading-frame, websocket-messages-accept, websocket-frame-headers |
| `http/internal/websocket_messages` | websocket-messages-accept, websocket-frame-headers |
| `http/internal/websocket_output` | websocket-output-message, websocket-output-frames, websocket-output-tickets |
| `http/internal/websocket_reading` | websocket-reading-frame |
| `http/internal/websocket_upgrade` | websocket-upgrade-bytes, websocket-session-memory |
| `http/internal/wire_bytes` | http-wire-bytes |
| `http/media_type` | http-media-type, http-media-parameters, http-media-scanning |
| `http/message` | http-client-request, http-message-new, websocket-origin-allowed, http-cookie-client-context, http-handler-response, http2-fields, http1-message-writing-output, http1-session-response, http2-session-response, http-conversation-response, http-retry-response, http-redirect-response, http-cache-response, http-cache-validation-request, web-request-bodies, http-exchange-body |
| `http/method` | http-method-semantics |
| `http/negotiation` | http-quality, http-negotiation-quality, http-negotiation-codings, http-negotiation-charsets, http-negotiation-languages, http-negotiation-media |
| `http/proxy` | http-connection-policies |
| `http/proxy_connection` | http-connection-policies |
| `http/proxy_policy` | http-proxy-policy-url, http-environment-proxy, http-connection-policies |
| `http/proxy_request` | http-proxy-request, http-connection-policies |
| `http/proxy_tunnel` | http-proxy-tunnel-ready |
| `http/redirect` | http-redirect-response, http-exchange-decorators |
| `http/request_context` | http-request-context-parameters |
| `http/retry` | http-retry-response, http-exchange-decorators |
| `http/robots` | http-robots-allowed, http-robots-fetch |
| `http/routing` | http-routing-match, web-routing-routes, http-routing-dispatch |
| `http/selected_connection` | http-connection-policies |
| `http/server` | — |
| `http/server_policy` | http-server-policy-handling-timeout, http-policy-protocols |
| `http/sse` | http-sse-retry, http-sse-read, http-sse-body |
| `http/static` | http-static-indexes, http-static-files |
| `http/status` | http-status-phrase, http-handler-response, http1-session-response, http2-session-response, http-conversation-response, web-routing-dispatch |
| `http/target` | http-request-target, http-client-request, http1-head, http-target-forms, http-target-kind-errors |
| `http/tls_session` | http-tls-session-version |
| `http/token` | http-token-string |
| `http/transport_observation` | http-transport-observation-begin |
| `http/version` | http-version |
| `http/websocket` | websocket-text, websocket-server-protocols, websocket-client-limits, websocket-messages-accept, websocket-output-message, websocket-frame-headers |
| `http/websocket_client` | websocket-client-limits, websocket-handshake-pair, websocket-client-upgrade |
| `http/websocket_handshake` | websocket-handshake-string, websocket-handshake-pair |
| `http/websocket_origin` | websocket-origin-allowed, websocket-server-protocols |
| `http/websocket_server` | websocket-server-protocols, websocket-server-memory |
| `http/websocket_session` | websocket-session-memory |
| `http/xmlrpc` | http-xmlrpc-bytes, http-xmlrpc-containers, http-xmlrpc-exchange, xmlrpc-float-print |
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
| `json` | json-parse, json-round-trip, json-render, template-render, html-template, template-json-value, template-sections, jwt-hs256-sign, json-log-line, json-node-reads, json-node-shape, json-maybe-reader, json-maybe-presence, json-presence-reader, json-streaming |
| `locale/case` | locale-case, locale-case-currency |
| `locale/collation` | — |
| `locale/currency` | currency-digits, locale-case-currency |
| `locale/data` | locale-fallbacks, locale-fields |
| `locale/date_time` | localized-date, locale-gregorian-names, locale-localized-times, locale-gmt-offset |
| `locale/internal/currencies` | cldr-tables, locale-cldr-numbers |
| `locale/internal/day_periods` | — |
| `locale/internal/gregorian` | locale-cldr-gregorian |
| `locale/internal/iana` | cldr-tables, locale-cldr-gregorian, locale-cldr-numbers |
| `locale/internal/likely` | cldr-tables |
| `locale/internal/lists` | locale-cldr-gregorian |
| `locale/internal/numbers` | cldr-tables, locale-cldr-numbers |
| `locale/internal/parents` | cldr-tables |
| `locale/internal/plurals` | locale-cldr-gregorian |
| `locale/internal/zone_meta` | cldr-tables |
| `locale/internal/zone_names` | — |
| `locale/likely` | likely-locale |
| `locale/list` | locale-list |
| `locale/match` | basic-language-range, locale-fields, locale-filtered |
| `locale/number` | locale-decimal, locale-number-print, locale-number-data, locale-number-magnitude, locale-number-printing, locale-number-zeroes |
| `locale/system` | language-preferences, locale-preferred |
| `locale/tag` | locale-case, plural-ru, locale-decimal, locale-list, locale-subtag-shapes, locale-subtag-keys, locale-extensions, locale-fields |
| `logging/enrichment` | log-pipeline |
| `logging/field` | json-log-line, logging-names-fields |
| `logging/file` | log-file, logging-files |
| `logging/filter` | log-pipeline, logging-writers, logging-names-fields |
| `logging/level` | logger-levels, logging-lines |
| `logging/line` | json-log-line, logging-lines |
| `logging/logger` | logger-levels, logging-lines, logging-logger-rotation |
| `logging/name` | json-log-line, logging-names-fields |
| `logging/named` | log-pipeline |
| `logging/queue` | queued-logs, logging-logger-rotation |
| `logging/record` | json-log-line, logging-lines |
| `logging/redact` | log-pipeline, logging-writers, logging-lines |
| `logging/rotation` | size-rotation, logging-logger-rotation, logging-rotating-file |
| `logging/stream` | json-stream-logs, logging-lines |
| `logging/syslog` | — |
| `logging/write` | log-pipeline, logger-levels, logging-writers |
| `mail/imap` | imap-name, imap-capabilities-construct |
| `mail/imap_command` | imap-command-bytes, imap-exchange-perform, imap-session-search, imap-session-append |
| `mail/imap_command_reading` | imap-command-bytes, imap-command-reading-string, imap-state-messages, imap-exchange-perform, imap-command-input |
| `mail/imap_data` | imap-data-tokens, imap-data-values |
| `mail/imap_exchange` | imap-exchange-perform, imap-session-select, imap-session-append, imap-idle-cancellation |
| `mail/imap_message` | imap-message-uid, imap-fetch-fields |
| `mail/imap_metadata` | imap-metadata-uids, imap-status-metadata, imap-fetch-fields |
| `mail/imap_name` | imap-command-bytes, imap-name-wire-name, imap-session-expire, imap-exchange-perform, imap-name-decoded |
| `mail/imap_reading` | imap-reading-read, mail-reply-snapshots |
| `mail/imap_response` | imap-response, imap-message-uid, imap-state-messages, imap-response-decoder, imap-response-progress |
| `mail/imap_search` | imap-search-key, imap-search-operators |
| `mail/imap_session` | imap-session-expire, imap-session-select, imap-session-examine, imap-session-mailboxes, imap-session-search, imap-session-copy, imap-session-append, imap-session-events, imap-idle-cancellation, imap-copy-mapping |
| `mail/imap_state` | imap-state-messages, imap-session-events |
| `mail/imap_uid` | imap-metadata-uids, imap-uid-argument, imap-message-uid, imap-uid-scope |
| `mail/mailbox` | mail-mailbox-membership, mail-flags-stored, mail-maildir-files |
| `mail/mbox` | mbox-string, mail-mbox-roundtrip |
| `mail/mbox_store` | mail-mbox-roundtrip |
| `mail/message` | mail-message-string, smtp-data-bytes, mail-message-values |
| `mail/pop3` | pop3-size, pop3-session-bytes, pop3-listing |
| `mail/pop3_reading` | pop3-reading-read, mail-reply-snapshots |
| `mail/pop3_reply` | pop3-size, pop3-reply, pop3-reply-decoder |
| `mail/pop3_session` | pop3-session-bytes, pop3-session-list, pop3-session-mail |
| `mail/smtp` | smtp-maximum-size, smtp-data-bytes, smtp-client-settings |
| `mail/smtp_data` | smtp-data-bytes, smtp-client-settings |
| `mail/smtp_reading` | smtp-reading-read, mail-reply-snapshots |
| `mail/smtp_reply` | smtp-maximum-size, smtp-reply-fed, smtp-data-bytes, smtp-reply-tail |
| `mail/smtp_session` | smtp-session-response, smtp-session-submit, smtp-client-settings |
| `math/aggregate` | aggregate-extremes |
| `math/constants` | — |
| `math/decimal` | locale-decimal, decimal-plus, shortest-float |
| `math/rounding` | — |
| `message/header` | message-header |
| `mime/disposition` | mime-disposition |
| `mime/media_type` | mime-media-type, mime-extension, mime-parameters, mime-scanning |
| `mime/part` | mime-text-body, mime-part-build, mime-parse, mime-validate |
| `mime/transfer` | mime-transfer, mime-validate |
| `mime/types` | mime-extension, mime-types-map, mime-system-types |
| `mime/word` | mime-encoded-word, mime-scanning |
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
| `os/command` | command-string, os-environment-read |
| `os/command_capture` | process-capture |
| `os/directory` | host-facts |
| `os/environment` | http-environment-proxy, environment-values, fs-temporary-tree, os-environment-read, os-search-path |
| `os/pipeline_capture` | process-capture |
| `os/platform` | host-facts, os-environment-read, os-search-path |
| `os/process` | process-exit, os-process-pipe, os-process-readers, os-process-streams, os-process-misc |
| `os/run` | process-capture, os-process-misc |
| `os/signal` | — |
| `os/storage` | — |
| `parallel/map` | parallel-map |
| `plist/parse` | plist-parse, plist-xml-source, plist-xml-text |
| `plist/value` | plist-parse, plist-integer, plist-write, plist-xml-text, plist-dictionary-reads, plist-dictionary-maybe, plist-maybe-more |
| `plist/write` | plist-write |
| `random/distribution` | seeded-uniform |
| `random/source` | pcg-bytes, seeded-uniform, websocket-output-message |
| `regex/escape` | regex-escape |
| `regex/match` | regex-capture, regex-observation, regex-match-groups |
| `regex/pattern` | regex-replace, regex-pattern-queries, regex-observation |
| `regex/range` | regex-range |
| `regex/replace` | regex-replace, regex-replace-forms |
| `regex/search` | regex-capture, regex-first-match, regex-prefix-match, regex-entire-match, regex-replace-forms |
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
| `testing/report` | test-reports |
| `testing/result` | test-report-counts |
| `testing/runner` | test-reports |
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
| `toml/value` | toml-parse, toml-table-scalars, toml-table-lists, toml-temporal, toml-maybe-scalars, toml-values-built |
| `unicode/character` | unicode-category |
| `unicode/normalization` | normalize-nfc |
| `url/form` | form-parse, url-form |
| `url/percent` | percent-encode, percent-decode, url-form |
| `url/query` | query-parse, form-parse, http-form-parameters, url-query, url-form |
| `url/value` | url-parts, http-exchange-string, http-proxy-policy-url, http-proxy-request, http-robots-allowed, websocket-origin-allowed, http-cookie-client-context, http-environment-proxy, http-endpoint-allowed, http-cookie-jar-store, http-proxy-tunnel-ready, url-parts-read, url-parts-edit |
| `uuid/generation` | uuid5-name, uuid-named, uuid-custom |
| `uuid/value` | uuid-parse, uuid-custom, uuid-text |
| `web/app` | — |
| `web/around` | web-around-routes, web-around-auth, web-routing-dispatch |
| `web/exception` | web-exception-response, web-routing-dispatch |
| `web/internal/reply` | web-internal-reply-response, web-reply-fields |
| `web/reply` | web-reply-response, web-testing-get, web-reply-html, web-reply-json, web-reply-created, web-reply-bytes, web-reply-stream, web-reply-empty, http-sse-body |
| `web/request` | web-request-link, web-json-readers, web-json-optionals, web-request-values, web-request-bodies |
| `web/routing` | web-routing-routes, web-testing-get, web-routing-methods, http-static-files, web-routing-dispatch |
| `web/session` | web-session-text, web-session-memory, web-session-lifecycle |
| `web/testing` | web-testing-get, web-testing-methods |
| `xml/internal/events` | xml-events |
| `xml/name` | xml-name, xml-events |
| `xml/node` | xml-attribute, xml-node-build, xml-element |
| `xml/parse` | xml-attribute |
| `xml/write` | xml-escape, xml-events |
