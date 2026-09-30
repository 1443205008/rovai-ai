# Camp and history

Choose the narrowest scope that answers the question:

| Need | Command |
| --- | --- |
| Find accessible Camps or a Camp ID | `rovai camp list --help` |
| Search the current Camp | `rovai camp search --query "amount"` |
| Search a known historical Camp | `rovai camp search --camp-id <camp-id> --query "amount"` |
| Read an exact message or a timeline/thread page | `rovai camp read --help` |
| Find a message whose Camp is unknown | `rovai history search --help` |

## Read forms

Bare `rovai camp read` returns the latest 20 visible messages in the current Camp. `--camp-id` changes only the target Camp.

```bash
rovai camp read --limit 20
rovai camp read --before <nextCursor>
rovai camp read --message-id <message-id>
rovai camp read --thread <message-id> --limit 20
```

Timeline and thread pages move from the latest message or anchor toward older messages. Continue with the returned `nextCursor` as `--before`. Exact `--message-id` reads return the full message and cannot combine with `--thread`, `--before` or `--limit`. There are no mode or direction fields.

Search/read resolve one Camp: omitted scope means the current Camp; an explicit historical target must belong to the current Run's frozen access scope and remain accessible. An explicit current Camp ID is equivalent to omission. A message ID alone does not search across Camps.

When the Camp is unknown, use history search to obtain `campId` and `messageId`, then read that exact pair. When the Camp is known, search there if needed, then read the exact message. Inspect the exact item's `addressing` when recipients or Principal mentions matter; snippets are discovery aids.

Cross-Camp search requires a real need for wider history. An uncertain mutation outcome follows [Recovery](recovery.md); similar text, author or time cannot prove invocation identity. Send always uses the authenticated current Camp and accepts no caller-supplied Camp ID.
