# Plan: comments on agent replies

Build a small app that shows an agent's reply and lets me answer parts of it. The reply is
Markdown. I read it with Vim's keys, select a passage and leave a comment under it.

## What a comment holds

Each comment stores the quote and its lines, so the feedback still makes sense when the agent
has moved on. There is also one general comment for the reply as a whole.

1. Select with `v` or `V`, then press `c`.
2. Type the comment. Enter saves and Escape cancels.
3. Quit with `ZZ` and the review goes to stdout.

## Removing

To ask for something to go, select it and press `d`. The feedback quotes the Markdown to cut,
as written. A removal can carry a note of its own.

## Later

- Sharing a review by link
- Replies to comments
- An embedded terminal running Neovim for the comment box

```rust
fn feedback(review: &Review) -> String {
    review.comments.iter().map(|c| c.to_markdown()).collect()
}
```
