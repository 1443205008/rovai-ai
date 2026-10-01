# Thread and history

Choose the narrowest scope that answers the question:

| Need | Command |
| --- | --- |
| Find accessible Threads or a Thread ID | `rovai thread list --help` |
| Search the current Thread | `rovai thread search --query "amount"` |
| Search a known historical Thread | `rovai thread search --thread-id <thread-id> --query "amount"` |
| Read an exact message or a timeline/reply chain page | `rovai thread read --help` |
| Find a message whose Thread is unknown | `rovai history search --help` |

## Read forms

Bare `rovai thread read` returns the latest 20 visible messages in the current Thread. `--thread-id` changes only the target Thread.

```bash
rovai thread read --limit 20
rovai thread read --before <nextCursor>
rovai thread read --message-id <message-id>
rovai thread read --reply-chain <message-id> --limit 20
```

Timeline and reply chain pages move from the latest message or anchor toward older messages. Continue with the returned `nextCursor` as `--before`. Exact `--message-id` reads return the full message and cannot combine with `--reply-chain`, `--before` or `--limit`. There are no mode or direction fields.

Search/read resolve one Thread: omitted scope means the current Thread; an explicit historical target must belong to the current Run's frozen access scope and remain accessible. An explicit current Thread ID is equivalent to omission. A message ID alone does not search across Threads.

When the Thread is unknown, use history search to obtain `threadId` and `messageId`, then read that exact pair. When the Thread is known, search there if needed, then read the exact message. Inspect the exact item's `addressing` when recipients or Principal mentions matter; snippets are discovery aids.

Cross-Thread search requires a real need for wider history. An uncertain mutation outcome follows [Recovery](recovery.md); similar text, author or time cannot prove invocation identity. Send always uses the authenticated current Thread and accepts no caller-supplied Thread ID.
