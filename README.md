
# Wire — A From-Scratch Userspace TCP/IP Stack in Rust

A complete, zero-syscall-core userspace TCP/IP network stack implemented from scratch in Rust over a Linux TAP device. Operates directly on raw Layer 2 Ethernet frames with zero dependency on the Linux kernel's networking subsystem.

Capable of completing active and passive 3-way handshakes with real Linux hosts, executing application-level HTTP transfers, and reliably delivering data under simulated packet loss, duplication, and reordering.

---

## Key Highlights

- **Zero-I/O Core Architecture (`wire-core`):** The protocol state machine is decoupled from I/O and syscalls. It consumes byte slices and generates egress frames deterministically, making it 100% testable in deterministic simulation.
- **Full 11-State TCP Connection State Machine:** RFC 9293 compliant (`CLOSED`, `LISTEN`, `SYN-SENT`, `SYN-RECEIVED`, `ESTABLISHED`, `FIN-WAIT-1`, `FIN-WAIT-2`, `CLOSE-WAIT`, `CLOSING`, `LAST-ACK`, `TIME-WAIT`).
- **Sliding-Window Flow & Congestion Control:** Implements dynamic send/receive window sizing, Reno congestion control (slow start, congestion avoidance, fast retransmit on $3\times$ duplicate ACKs, fast recovery), and adaptive RTO calculation (RFC 6298 Jacobson/Karels with Karn's algorithm).
- **Deterministic Chaos Simulation (`wire-sim`):** Validated across 300 randomized topologies (100% pass rate) under 5% packet loss and 2% packet duplication using seeded PRNG execution.
- **Real-World Interoperability:** Tested against the Linux kernel network stack for both passive open (`wire-echo` talking to `nc`) and active open (`wire-httpget` talking to a Python HTTP server).

---

## Performance & Benchmarks

To evaluate the efficiency of the zero-copy, zero-syscall design, a dedicated high-performance benchmarking binary (`wire-perf`) was implemented to measure overhead across three key planes:

1. **RFC 1071 Checksum Throughput:** Measures the raw processing speed of one's complement addition over standard 1460-byte MTU segments.
2. **Core FSM Ingress Processing Rate:** Evaluates the throughput of packet ingestion, pseudo-header verification, IP/TCP header parsing, and connection state routing.
3. **Simulation Engine Processing Rate:** Measures ticks per second processed inside our chaos framework under clean, 1% lossy, and extreme lossy/duplicative conditions.

<p align="center">
  <img src="wire_benchmark.png" alt="Wire TCP/IP Stack Benchmark Results" width="100%">
</p>

### Performance Highlights

- **Word-Parallel Checksumming:** Our checksum processing achieves **>40 Gbps** throughput on standard consumer hardware, processing 1460-byte MTU segments in less than **250 ns**.
- **Ultra-Low-Latency FSM Routing:** Core state transition and TCP connection mapping take less than **25 ns per segment**, enabling an ingress packet processing rate of over **40 Million packets per second (Mpps)** on a single core.
- **Microsecond Simulation Loop:** The deterministic in-memory simulator drives the entire network stack's FSM forward at rates exceeding **30 million ticks/sec** on a single thread under network loss and jitter conditions.

---

## Architectural Layout

```text
              +-----------------------------------+
              |        Application Layer          |
              |     (wire-echo / wire-httpget)    |
              +-----------------+-----------------+
                                | tcp_send / tcp_recv
              +-----------------v-----------------+
              |             wire-core             |
              |   - TCP 11-State FSM              |
              |   - Reno Congestion Control       |
              |   - Jacobson/Karels RTO & Karn    |
              |   - IPv4 Checksums & Routing      |
              |   - ARP Cache & Dynamic Resolve   |
              +--------+-----------------+--------+
                       |                 |
         Frame In / Out|                 |Frame In / Out
+----------------------v---+   +---------v--------------------+
|         wire-tap         |   |           wire-sim           |
| Linux /dev/net/tun Driver|   |   Deterministic In-Memory    |
| (IFF_TAP | IFF_NO_PI)    |   | Chaos Wire (Loss, Dup, Jitt) |
+--------------+-----------+   +------------------------------+
               |
  Raw L2 Frames|
               v
+------------------------------+
|   Linux Kernel Host (tap0)   |
|         192.168.99.1         |
+------------------------------+

```

---

## Protocol Coverage Matrix

| Layer | Protocol | Specification | Status | Key Features Handled |
| --- | --- | --- | --- | --- |
| **L2** | Ethernet II | IEEE 802.3 | **Complete** | MAC filtering, EtherType demux (`0x0800`, `0x0806`). |
| **L2.5** | ARP | RFC 826 | **Complete** | Request broadcast, reply handling, dynamic ARP bootstrap for active open. |
| **L3** | IPv4 | RFC 791 | **Complete** | 20-byte header parsing, one's complement checksum validation, TTL enforcement. |
| **L3.5** | ICMP | RFC 792 | **Complete** | Echo Request / Echo Reply (`ping` support with identifier preservation). |
| **L4** | TCP | RFC 9293 | **Complete** | 11-state FSM, 32-bit modular sequence arithmetic, pseudo-header checksum. |
| **L4** | Congestion | RFC 5681 | **Complete** | Slow start, congestion avoidance, fast retransmit, fast recovery. |
| **L4** | Timer / RTO | RFC 6298 | **Complete** | SRTT / RTTVAR calculation, exponential backoff, Karn's algorithm. |

---

## Project Structure

```text
.
├── Cargo.toml          # Workspace manifest
├── wire_benchmark.png  # Performance evaluation screenshot
├── scripts
│   ├── tap-up.sh       # Creates and configures tap0 interface
│   └── tap-down.sh     # Cleans up tap0 interface
├── wire-core           # Pure state engine (Zero I/O, no_std friendly)
│   └── src/lib.rs
├── wire-tap            # Linux TAP device ioctl driver
│   └── src/lib.rs
├── wire-sim            # Deterministic chaos simulator (PRNG-seeded)
│   ├── src/lib.rs
│   └── src/main.rs
└── wire-echo           # Integration binaries
    ├── src/main.rs     # Passive Open (Echo server on :8080)
    ├── src/bin/perf.rs # Benchmark suite
    └── src/bin/httpget.rs # Active Open (HTTP/1.1 client)

```

---

## Running & Verification

### Prerequisites (Arch / Debian / Fedora)

```bash
sudo modprobe tun

```

### 1. Run the Benchmark Suite

```bash
cargo run --release --bin wire-perf

```

### 2. Run the Deterministic Chaos Simulator

The simulator executes 300 test runs across clean, lossy, and duplication-heavy topologies with zero physical hardware requirements:

```bash
cargo run --release -p wire-sim

```

Expected Output:

```text
[clean] 100/100 passed (0 failed)
[1% loss] 100/100 passed (0 failed)
[5% loss + 2% dup] 100/100 passed (0 failed)

```

### 3. Demo A: Passive Open Echo Server (`nc` → `wire-echo`)

Bring up the virtual TAP interface:

```bash
./scripts/tap-up.sh

```

Start the userspace stack:

```bash
cargo run --release --bin wire-echo

```

In a separate terminal, stream 1MB of random data from the Linux host through the stack:

```bash
dd if=/dev/urandom of=/tmp/wire_input.bin bs=1K count=1024 2>/dev/null
sha256sum /tmp/wire_input.bin
nc -q 2 192.168.99.2 8080 < /tmp/wire_input.bin > /tmp/wire_output.bin
sha256sum /tmp/wire_output.bin

```

Both SHA-256 hashes will match byte-for-byte, verifying zero data corruption.

### 4. Demo B: Active Open HTTP Client (`wire-httpget` → Kernel Python Server)

Start an HTTP server on your host machine bound to `192.168.99.1`:

```bash
echo "Hello from your real Host over userspace TCP!" > /tmp/index.html
cd /tmp && python3 -m http.server 8000 --bind 192.168.99.1

```

Execute the userspace client:

```bash
cargo run --release --bin wire-httpget

```

Output:

```text
⚡ Starting Active Connection to 192.168.99.1:8000
✅ Connection established! Sending HTTP GET...
📥 Server closed connection (FIN received). Closing our side.
🛑 Connection closed cleanly by stack.
=== HTTP RESPONSE RECEIVED ===
HTTP/1.0 200 OK
Server: SimpleHTTP/0.6 Python/3.14.7
Date: Thu, 17 Sep 2026 07:32:13 GMT
Content-type: text/html
Content-Length: 51
Last-Modified: Thu, 17 Sep 2026 07:31:57 GMT

Hello from your real Host over userspace TCP!
==============================

```

---

## Deep-Dive Protocol Nuances Handled

* **Modular Sequence Arithmetic:** Comparisons use cyclic sequence space `(((a - b) as i32) < 0)`, avoiding sequence wraparound bugs near $2^{32} - 1$.
* **Karn's Algorithm:** RTT samples are strictly ignored for retransmitted segments to prevent RTT estimator poisoning.
* **Dynamic ARP Bootstrapping:** In active open mode, the stack detects unresolved MAC destinations, broadcasts an ARP request, drops the initial frame, and allows the RTO timer to retransmit the SYN once the ARP reply populates the L2 cache.
* **Checksum Zeroing:** TCP segment retransmissions explicitly zero out the checksum offset before calculating the pseudo-header checksum, avoiding mathematical sum corruption on retried packets.

---

## License

MIT / Apache 2.0

```

