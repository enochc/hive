# Performance notes

Hive is meant to be a fast, efficient cross-service data communication
layer, that's the top priority whenever touching this crate. This file
tracks concrete inefficiencies spotted in passing while working on other
things. Not a design doc, just a punch list with enough detail to act on,
resolved items get removed rather than kept as a changelog, git history is
the record of what happened.

## Open

### `read_loop` allocates a fresh `Vec<u8>` per incoming message (peer.rs:480)

```rust
let mut size_buff = vec![0u8; message_size as usize];
```

One heap allocation per message payload, on the read hot path. Not fixed
yet: a real fix needs a reusable/pooled buffer, and the message's bytes get
handed off to another task via an unbounded channel (`SocketEvent::Message`)
that can hold onto them indefinitely, so a naive shared buffer can't just
be reused in place, it needs actual pool lifecycle management (something
returns buffers once the receiver is done with them). That's a bigger,
riskier change than a quick fix, worth doing as its own dedicated pass
rather than folded into an unrelated task.
