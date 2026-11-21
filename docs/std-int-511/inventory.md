# Public BigInt inventory for #511

Snapshot: fresh release/062 worktree supplied on 2026-11-22. Counts are directly declared public signatures; inherited role slots are not counted twice. Hidden declarations/actions and fields are excluded from the public counts. Generic containers are included.

- 104 public actions: 83 return BigInt (including containers), 28 accept it; 7 do both.
- 32 constructors accept BigInt in 41 parameter type positions.
- Combined: 136 signatures, 83 results, 60 input signatures (69 input type positions).
- Migrate: 93 actions and 32 constructors (125 signatures). Retain: 11 BigInt actions (10 result signatures, 8 input signatures).
- Four constructor overloads merge with existing int overloads: Json(BigInt), JsonField(name, BigInt), DbInteger(BigInt), DbInteger(Maybe<BigInt>). No object is removed.

BigInt itself retains both constructors (string/int), its fields, Digits cap, errors and seven operator/ordering role instantiations. Its 11 signatures below remain type-specific: arithmetic returns the explicit wrapper, comparison accepts its own type, and power/gcd/lcm are numeric algorithms. The int-based BoundedPower lives alongside it and is shared with BigInt.power. No other public signature still mentions the BigInt type.

| Module / declaration | Old signature | New signature | Classification |
|---|---|---|---|
| archive/metadata.ok / ArchiveOwnership | `user_identity() is BigInt` | `user_identity() is int` | migrate |
| archive/metadata.ok / ArchiveOwnership | `group_identity() is BigInt` | `group_identity() is int` | migrate |
| archive/metadata.ok / ArchiveOwnership | `ArchiveOwnership(user is string , group is string , user_identity is BigInt , group_identity is BigInt)` | `ArchiveOwnership(user is string , group is string , user_identity is int , group_identity is int)` | migrate |
| db/read.ok / DbColumnReader | `integer() is BigInt` | `integer() is int` | migrate |
| db/read.ok / DbPresenceReader | `integer() is Presence < BigInt >` | `integer() is Presence < int >` | migrate |
| db/read.ok / DbMaybeReader | `integer() is Maybe < BigInt >` | `integer() is Maybe < int >` | migrate |
| db/row.ok / Row | `integer(name is string) is BigInt` | `integer(name is string) is int` | migrate |
| db/sqlite/backup.ok / BackupReport | `copies() is BigInt` | `copies() is int` | migrate |
| db/sqlite/backup.ok / BackupReport | `total() is BigInt` | `total() is int` | migrate |
| db/sqlite/backup.ok / BackupReport | `BackupReport(copies is BigInt , total is BigInt)` | `BackupReport(copies is int , total is int)` | migrate |
| db/sqlite/blob.ok / SqliteBlobOpening | `opened_blob(database is string , table is string , column is string , row is BigInt , writable is bool) is SqliteBlob` | `opened_blob(database is string , table is string , column is string , row is int , writable is bool) is SqliteBlob` | migrate |
| db/sqlite/blob.ok / SqliteBlob | `bytes(offset is BigInt , limit is int) is @ byte` | `bytes(offset is int , limit is int) is @ byte` | migrate |
| db/sqlite/blob.ok / SqliteBlob | `write(offset is BigInt , source is @ byte) is SqliteBlob` | `write(offset is int , source is @ byte) is SqliteBlob` | migrate |
| db/sqlite/blob.ok / OpenedSqliteBlob | `OpenedSqliteBlob(origin is SqliteBlobOpening , database is string , table is string , column is string , row is BigInt , writable is bool)` | `OpenedSqliteBlob(origin is SqliteBlobOpening , database is string , table is string , column is string , row is int , writable is bool)` | migrate |
| db/sqlite.ok / SqliteConnection | `opened_blob(database is string , table is string , column is string , row is BigInt , writable is bool) is SqliteBlob` | `opened_blob(database is string , table is string , column is string , row is int , writable is bool) is SqliteBlob` | migrate |
| db/sqlite.ok / SqliteSession | `open_blob(database is string , table is string , column is string , row is BigInt , writable is bool , token is int) is SqliteBlob` | `open_blob(database is string , table is string , column is string , row is int , writable is bool , token is int) is SqliteBlob` | migrate |
| db/sqlite.ok / SqliteSession | `blob_bytes(id is int , offset is BigInt , limit is int , token is int) is @ byte` | `blob_bytes(id is int , offset is int , limit is int , token is int) is @ byte` | migrate |
| db/sqlite.ok / SqliteSession | `blob_write(id is int , offset is BigInt , source is @ byte , token is int) is bool` | `blob_write(id is int , offset is int , source is @ byte , token is int) is bool` | migrate |
| db/sqlite.ok / SqliteFrame | `copies() is BigInt` | `copies() is int` | migrate |
| db/sqlite.ok / SqliteFrame | `total() is BigInt` | `total() is int` | migrate |
| db/sqlite.ok / SqliteTransaction | `opened_blob(database is string , table is string , column is string , row is BigInt , writable is bool) is SqliteBlob` | `opened_blob(database is string , table is string , column is string , row is int , writable is bool) is SqliteBlob` | migrate |
| db/statement.ok / Execution | `affected() is Maybe < BigInt >` | `affected() is Maybe < int >` | migrate |
| db/statement.ok / Execution | `affected_rows() is BigInt` | `affected_rows() is int` | migrate |
| db/statement.ok / Execution | `Execution(affected is BigInt)` | `Execution(affected is int)` | migrate |
| db/value.ok / DbValueWriting | `integer(value is BigInt) is void` | `integer(value is int) is void` | migrate |
| db/value.ok / DbInteger | `integer() is BigInt` | `integer() is int` | migrate |
| db/value.ok / DbInteger | `DbInteger(value is BigInt)` | `DbInteger(value is int)` | migrate |
| db/value.ok / DbInteger | `DbInteger(value is Maybe < BigInt >)` | `DbInteger(value is Maybe < int >)` | migrate |
| db/value.ok / ObservedDbValue | `integer() is BigInt` | `integer() is int` | migrate |
| db/value.ok / DbValueCapture | `integer(value is BigInt) is void` | `integer(value is int) is void` | migrate |
| db/value.ok / DbValueCapture | `integer_value() is BigInt` | `integer_value() is int` | migrate |
| fs/stat.ok / FileIdentity | `inode() is BigInt` | `inode() is int` | migrate |
| fs/stat.ok / FileIdentity | `FileIdentity(device is int , inode is BigInt)` | `FileIdentity(device is int , inode is int)` | migrate |
| http/body.ok / BodyOpening | `length() is BigInt` | `length() is int` | migrate |
| http/body.ok / ByteBody | `length() is BigInt` | `length() is int` | migrate |
| http/body.ok / EmptyBody | `length() is BigInt` | `length() is int` | migrate |
| http/body.ok / OneShotBody | `length() is BigInt` | `length() is int` | migrate |
| http/body_coding.ok / GzipEncodedBody | `length() is BigInt` | `length() is int` | migrate |
| http/body_coding.ok / GzipDecodedBody | `length() is BigInt` | `length() is int` | migrate |
| http/body_coding.ok / BrotliEncodedBody | `length() is BigInt` | `length() is int` | migrate |
| http/body_coding.ok / BrotliDecodedBody | `length() is BigInt` | `length() is int` | migrate |
| http/cache_memory.ok / MemoryHttpCache | `MemoryHttpCache(clock is Timing , maximum is BigInt)` | `MemoryHttpCache(clock is Timing , maximum is int)` | migrate |
| http/cache_memory.ok / MemoryHttpCache | `MemoryHttpCache(clock is Timing , maximum is BigInt , authenticated is bool)` | `MemoryHttpCache(clock is Timing , maximum is int , authenticated is bool)` | migrate |
| http/client_policy.ok / HttpLimits | `body_bytes() is BigInt` | `body_bytes() is int` | migrate |
| http/client_policy.ok / HttpLimits | `HttpLimits(header_bytes is int , line_bytes is int , body_bytes is BigInt)` | `HttpLimits(header_bytes is int , line_bytes is int , body_bytes is int)` | migrate |
| http/conditional.ok / ByteRange | `start() is BigInt` | `start() is int` | migrate |
| http/conditional.ok / ByteRange | `end() is BigInt` | `end() is int` | migrate |
| http/conditional.ok / ByteRange | `size() is BigInt` | `size() is int` | migrate |
| http/conditional.ok / ByteRange | `ByteRange(start is BigInt , end is BigInt)` | `ByteRange(start is int , end is int)` | migrate |
| http/conditional.ok / ByteRanges | `satisfiable(size is BigInt) is bool` | `satisfiable(size is int) is bool` | migrate |
| http/conditional.ok / ByteRanges | `ranges(size is BigInt) is Sequencing < ByteRange >` | `ranges(size is int) is Sequencing < ByteRange >` | migrate |
| http/form.ok / FormLimits | `field_bytes() is BigInt` | `field_bytes() is int` | migrate |
| http/form.ok / FormLimits | `file_bytes() is BigInt` | `file_bytes() is int` | migrate |
| http/form.ok / FormLimits | `total_bytes() is BigInt` | `total_bytes() is int` | migrate |
| http/form.ok / FormLimits | `FormLimits(fields is int , files is int , field_bytes is BigInt , file_bytes is BigInt , total_bytes is BigInt)` | `FormLimits(fields is int , files is int , field_bytes is int , file_bytes is int , total_bytes is int)` | migrate |
| http/http1_body.ok / Http1BodyReading | `length() is BigInt` | `length() is int` | migrate |
| http/http1_body.ok / Http1IncomingBody | `length() is BigInt` | `length() is int` | migrate |
| http/http1_body_check.ok / Http1BodyCheck | `known() is @ BigInt` | `known() is @ int` | migrate |
| http/http1_chunk.ok / ParsedHttp1ChunkLine | `size() is BigInt` | `size() is int` | migrate |
| http/http1_framing.ok / Http1BodyPlan | `length() is BigInt` | `length() is int` | migrate |
| http/http1_framing.ok / Http1BodyPlan | `Http1BodyPlan(kind is Http1BodyKind , count is BigInt , reusable is bool)` | `Http1BodyPlan(kind is Http1BodyKind , count is int , reusable is bool)` | migrate |
| http/http1_framing.ok / Http1FramingFields | `length() is @ BigInt` | `length() is @ int` | migrate |
| http/http1_framing.ok / Http1FramingFields | `fixed(length is BigInt , reuse is bool) is Http1BodyPlan` | `fixed(length is int , reuse is bool) is Http1BodyPlan` | migrate |
| http/internal/http2_body.ok / Http2BodyBuffer | `configure(expected is @ BigInt , empty is bool) is void` | `configure(expected is @ int , empty is bool) is void` | migrate |
| http/internal/http2_body.ok / Http2BodyBuffer | `Http2BodyBuffer(stream is int , initial is int , maximum is int , events is Http2BodyEvents , expected is @ BigInt , empty is bool)` | `Http2BodyBuffer(stream is int , initial is int , maximum is int , events is Http2BodyEvents , expected is @ int , empty is bool)` | migrate |
| http/internal/http2_body.ok / Http2IncomingBody | `length() is BigInt` | `length() is int` | migrate |
| http/internal/http2_body.ok / Http2IncomingBody | `Http2IncomingBody(origin is Http2BodyBuffer , known is @ BigInt)` | `Http2IncomingBody(origin is Http2BodyBuffer , known is @ int)` | migrate |
| http/internal/http2_body.ok / Http2BodyLength | `known() is @ BigInt` | `known() is @ int` | migrate |
| http/sse.ok / EventBody | `length() is BigInt` | `length() is int` | migrate |
| http/websocket.ok / WebSocketLimits | `message_bytes() is BigInt` | `message_bytes() is int` | migrate |
| http/websocket.ok / WebSocketLimits | `WebSocketLimits(message_bytes is BigInt , frame_bytes is int , queue is int)` | `WebSocketLimits(message_bytes is int , frame_bytes is int , queue is int)` | migrate |
| http/xmlrpc.ok / XmlRpcLimits | `text_bytes() is BigInt` | `text_bytes() is int` | migrate |
| http/xmlrpc.ok / XmlRpcLimits | `binary_bytes() is BigInt` | `binary_bytes() is int` | migrate |
| http/xmlrpc.ok / XmlRpcLimits | `XmlRpcLimits(depth is int , values is int , text_bytes is BigInt , binary_bytes is BigInt)` | `XmlRpcLimits(depth is int , values is int , text_bytes is int , binary_bytes is int)` | migrate |
| http/xmlrpc.ok / XmlRpcLimits | `XmlRpcLimits(depth is int , values is int , text_bytes is BigInt , binary_bytes is BigInt , nil_enabled is bool)` | `XmlRpcLimits(depth is int , values is int , text_bytes is int , binary_bytes is int , nil_enabled is bool)` | migrate |
| i18n/plural.ok / PluralNumber | `operand(name is string) is BigInt` | `operand(name is string) is int` | migrate |
| json.ok / Json | `integer() is BigInt` | `integer() is int` | migrate |
| json.ok / Json | `integer(name is string) is BigInt` | `integer(name is string) is int` | migrate |
| json.ok / Json | `Json(value is BigInt)` | `Json(value is int)` | migrate |
| json.ok / JsonField | `JsonField(name is string , value is BigInt)` | `JsonField(name is string , value is int)` | migrate |
| json.ok / JsonPresenceReader | `integer() is Presence < BigInt >` | `integer() is Presence < int >` | migrate |
| json.ok / JsonMaybeReader | `integer() is Maybe < BigInt >` | `integer() is Maybe < int >` | migrate |
| locale/number.ok / NumberMagnitude | `coefficient() is BigInt` | `coefficient() is int` | migrate |
| locale/number.ok / NumberMagnitude | `NumberMagnitude(coefficient is BigInt , exponent is rat , negative is bool)` | `NumberMagnitude(coefficient is int , exponent is rat , negative is bool)` | migrate |
| locale/number.ok / NumberQuantized | `coefficient() is BigInt` | `coefficient() is int` | migrate |
| locale/number.ok / LocalizedBigInteger | `LocalizedBigInteger(origin is BigInt , locale is Locale , format is NumberFormat)` | `LocalizedBigInteger(origin is int , locale is Locale , format is NumberFormat)` | migrate |
| locale/number.ok / LocalizedIntegerFromString | `integer() is BigInt` | `integer() is int` | migrate |
| logging/queue.ok / QueuedLogs | `dropped_records() is BigInt` | `dropped_records() is int` | migrate |
| logging/rotation.ok / Rotating | `needed(size is BigInt , opened is Instant , now is Instant) is bool` | `needed(size is int , opened is Instant , now is Instant) is bool` | migrate |
| logging/rotation.ok / SizeRotating | `needed(size is BigInt , opened is Instant , now is Instant) is bool` | `needed(size is int , opened is Instant , now is Instant) is bool` | migrate |
| logging/rotation.ok / SizeRotating | `SizeRotating(limit is BigInt)` | `SizeRotating(limit is int)` | migrate |
| logging/rotation.ok / IntervalRotating | `needed(size is BigInt , opened is Instant , now is Instant) is bool` | `needed(size is int , opened is Instant , now is Instant) is bool` | migrate |
| logging/syslog.ok / SyslogLogs | `dropped_records() is BigInt` | `dropped_records() is int` | migrate |
| logging/write.ok / BestEffortLogs | `failures() is BigInt` | `failures() is int` | migrate |
| mail/imap_session.ok / ImapSelectionToken | `generation() is BigInt` | `generation() is int` | migrate |
| mail/imap_session.ok / ImapSelectionToken | `ImapSelectionToken(name is ImapMailboxName , generation is BigInt)` | `ImapSelectionToken(name is ImapMailboxName , generation is int)` | migrate |
| mail/imap_uid.ok / ImapUid | `number() is BigInt` | `number() is int` | migrate |
| mail/imap_uid.ok / ImapUid | `ImapUid(origin is BigInt)` | `ImapUid(origin is int)` | migrate |
| mail/imap_uid.ok / ImapUidValidity | `number() is BigInt` | `number() is int` | migrate |
| mail/imap_uid.ok / ImapUidValidity | `ImapUidValidity(origin is BigInt)` | `ImapUidValidity(origin is int)` | migrate |
| mail/pop3.ok / PopMessage | `size() is BigInt` | `size() is int` | migrate |
| mail/pop3.ok / PopMessage | `PopMessage(number is PopNumber , size is BigInt , identifiers is @ PopIdentifier)` | `PopMessage(number is PopNumber , size is int , identifiers is @ PopIdentifier)` | migrate |
| mail/pop3.ok / PopMailboxState | `size() is BigInt` | `size() is int` | migrate |
| mail/smtp.ok / SmtpCapabilities | `maximum_size() is BigInt` | `maximum_size() is int` | migrate |
| math/decimal.ok / Decimal | `coefficient() is BigInt` | `coefficient() is int` | migrate |
| net/ip.ok / IpNetwork | `size() is BigInt` | `size() is int` | migrate |
| numbers/bigint.ok / BigInt | `order(other is BigInt) is Order` | `order(other is BigInt) is Order` | retain |
| numbers/bigint.ok / BigInt | `plus(other is BigInt) is BigInt` | `plus(other is BigInt) is BigInt` | retain |
| numbers/bigint.ok / BigInt | `minus(other is BigInt) is BigInt` | `minus(other is BigInt) is BigInt` | retain |
| numbers/bigint.ok / BigInt | `times(other is BigInt) is BigInt` | `times(other is BigInt) is BigInt` | retain |
| numbers/bigint.ok / BigInt | `divided(other is BigInt) is BigInt` | `divided(other is BigInt) is BigInt` | retain |
| numbers/bigint.ok / BigInt | `remainder(other is BigInt) is BigInt` | `remainder(other is BigInt) is BigInt` | retain |
| numbers/bigint.ok / BigInt | `negated() is BigInt` | `negated() is BigInt` | retain |
| numbers/bigint.ok / BigInt | `gcd(other is BigInt) is BigInt` | `gcd(other is BigInt) is BigInt` | retain |
| numbers/bigint.ok / BigInt | `lcm(other is BigInt) is BigInt` | `lcm(other is BigInt) is BigInt` | retain |
| numbers/bigint.ok / BigInt | `power(exponent is int) is BigInt` | `power(exponent is int) is BigInt` | retain |
| numbers/bigint.ok / BigInt | `power(exponent is int , maximum_bits is int) is BigInt` | `power(exponent is int , maximum_bits is int) is BigInt` | retain |
| numbers/bigrational.ok / BigRational | `numerator() is BigInt` | `numerator() is int` | migrate |
| numbers/bigrational.ok / BigRational | `denominator() is BigInt` | `denominator() is int` | migrate |
| numbers/bigrational.ok / BigRational | `BigRational(numerator is BigInt , denominator is BigInt)` | `BigRational(numerator is int , denominator is int)` | migrate |
| os/storage.ok / StorageUsage | `capacity() is BigInt` | `capacity() is int` | migrate |
| os/storage.ok / StorageUsage | `used() is BigInt` | `used() is int` | migrate |
| os/storage.ok / StorageUsage | `available() is BigInt` | `available() is int` | migrate |
| os/storage.ok / StorageUsage | `fits(amount is BigInt) is bool` | `fits(amount is int) is bool` | migrate |
| os/storage.ok / StorageUsage | `StorageUsage(capacity is BigInt , available is BigInt)` | `StorageUsage(capacity is int , available is int)` | migrate |
| plist/value.ok / PlistWriting | `integer(value is BigInt) is void` | `integer(value is int) is void` | migrate |
| plist/value.ok / PlistWriting | `uid(value is BigInt) is void` | `uid(value is int) is void` | migrate |
| plist/value.ok / PlistInteger | `integer() is BigInt` | `integer() is int` | migrate |
| plist/value.ok / PlistInteger | `PlistInteger(value is BigInt)` | `PlistInteger(value is int)` | migrate |
| plist/value.ok / PlistUid | `number() is BigInt` | `number() is int` | migrate |
| plist/value.ok / PlistUid | `PlistUid(value is BigInt)` | `PlistUid(value is int)` | migrate |
| testing/double.ok / Recorded | `index() is BigInt` | `index() is int` | migrate |
| testing/double.ok / Recorded | `Recorded(index is BigInt , value is T)` | `Recorded(index is int , value is T)` | migrate |
| time/duration.ok / Duration | `nanoseconds() is BigInt` | `nanoseconds() is int` | migrate |
| web/request.ok / RequestJson | `integer() is BigInt` | `integer() is int` | migrate |
| web/request.ok / RequestJson | `integer(name is string) is BigInt` | `integer(name is string) is int` | migrate |
