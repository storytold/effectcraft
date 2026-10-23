# Export output safety

Native exports encode into unique sibling files created by `tempfile` 3.27
(MIT OR Apache-2.0). After encoding completes and writers close, `TempPath::persist`
publishes an accepted replacement; `persist_noclobber` publishes a new destination
without overwriting an intervening creator. Publication uses the library's normal
platform replacement behavior. No audit privilege or custom metadata reconstruction
is required.

Cancellation and encoding errors leave existing media untouched and clean up only
job-owned staging paths. Replacement errors are returned with the destination and
publication count. A sequence publishes one complete file at a time: a later failure
can leave a mixture of complete old and new frames. There is no whole-sequence
transaction, rollback, trailing-frame deletion, or unrelated-file cleanup.

On Unix, new outputs use normal file creation mode (0666 filtered by the process
umask); replacements retain the existing file's mode bits. Staging is configured
through `tempfile::Builder::permissions`, with `File::set_permissions` retaining
replacement mode bits that the current umask would otherwise remove. Windows uses
ordinary creation security inherited from the directory. Ownership, ACLs, audit
entries, and extended attributes are not reconstructed. The library performs
replacement without syncing files or directories, so this is not a crash or
power-loss recovery protocol.

`preflight` inspects the precise movie, sequence, and overflow destinations without
creating files or folders. The host shows collisions and retains the authorization
with its queue/settings snapshot. `export_authorized` checks that reviewed paths,
file identities, lengths, timestamps, and basic permission metadata still match.
The default `export` refuses existing outputs without authorization. Skipped
sequence frames are not authorized for writes. Memory-sink exports keep their
existing callback contract. Collision lists are limited to 100000 existing outputs;
authorization metadata is limited to 64 MiB, counting its serialized size. Fresh
frames and overflow candidates do not consume the collision count limit.

The checks reject observed destination changes, and a process-local lock coordinates
publication between exports. An uncooperating external writer can still race the
last check and an accepted replacement; this is not a filesystem compare-and-swap
guarantee. Existing leaf symlinks resolve to their targets; replacing a hard-linked
name leaves its other names pointing at the previous file.

Render logs are separate, best-effort output. An existing log is preserved and a
unique sibling log is created; reports return the actual log path.
