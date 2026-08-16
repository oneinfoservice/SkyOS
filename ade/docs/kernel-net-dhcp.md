# Kernel net gate: DHCP lease on real hardware (K10)

**Status:** Spec for the kernel rewrite. Kernel tree is NOT modified by this
document. Landing evidence is the `tests/qemu_net_probe.exp` gate (K10 row in
`session-lifecycle.md` §6).

## The observed settlement (Aug 14, 2026)

Fresh headless boots of the working ISO (`release/skyos-probe.iso`, Aug 9
build) with `-device e1000,netdev=net0 -netdev user,id=net0` (QEMU SLIRP user
net, which serves DHCP on 10.0.2.2 and hands out 10.0.2.15) produce this
serial marker stream:

```
[BOOT] net init...
[E1000] sts=80080783 icr=00000002 rdh=0 rdt=31 cur=0 dd=0
[DHCP] lease lost
```

`[DHCP] configured IP:` **never appears**. The settlement: **smoltcp does not
deliver on hardware with the current kernel** — the stack initializes and the
DHCP socket polls exactly once (the `Event::Deconfigured` at
`kernel/src/net/mod.rs:148` is smoltcp's initial `config_changed=true` event),
but the E1000 RX/TX path never completes a DHCP exchange, so no lease is ever
obtained. The attempt is one-shot: there is no renew/retry loop, so the
failure is permanent for the boot.

## What is real vs broken

| Piece | Status | Source |
|---|---|---|
| E1000 device init + register dump | works (marker on serial) | `kernel/src/drivers/net/e1000.rs:244` |
| `net::init()` + smoltcp stack start | works | `kernel/src/main.rs:305` (`[BOOT] net init...`), `kernel/src/net/mod.rs:80` |
| IRQ → `net::poll()` → `iface.poll()` chain | wired, RX/TX still fails to deliver a lease | `kernel/src/net/mod.rs:92-114` |
| DHCP socket poll (one-shot) | runs (fires `Deconfigured`) | `kernel/src/net/mod.rs:118-152`, `kernel/src/net/dhcp.rs` |
| Lease acquisition | **broken** | no `[DHCP] configured IP:` ever |
| `Network: Stack initialized with DHCP (fallback IP 10.0.2.15, MAC: ...)` | **misleading** | `kernel/src/net/mod.rs:80` — printed via `crate::println!` (VGA-only, `vga_buffer.rs:164`, NOT serial-greppable), and the "fallback IP" is never assigned: `init()` only adds a default *route* to 10.0.2.2 (`net/mod.rs:74-75`), never the 10.0.2.15 address |

## Root-cause hypotheses (for the rewrite to investigate, in order)

1. **E1000 receive path**: the `[E1000]` dump shows `rdh=0 rdt=31` — the
   receive ring is not advancing. Verify RX descriptor completion, the
   `icr` interrupt bits (QEMU e1000 uses legacy IRQ + RX ring interrupts),
   and that `poll()` is actually called after a receive interrupt arrives.
2. **One-shot DHCP with no renew**: even a transient lost exchange never
   recovers. Add a `renew()` / retry loop in the DHCP socket driver (or
   re-`poll()` on a timer) so the first exchange failing is not fatal.
3. **Fallback-IP mislabel**: either assign 10.0.2.15 when the lease fails
   (making the message true) or delete the "fallback IP" text — the current
   message promises an address that is never configured.

## Landing condition (what K10 means)

- `[DHCP] configured IP:` appears in the serial log of a real boot with the
  e1000 device and QEMU user-mode networking.
- `tests/qemu_net_probe.exp` flips from `KERNEL-GATED:` to the hard
  `PASS: DHCP lease obtained on real hardware` verdict.
- The misleading `Network: Stack initialized ... fallback IP 10.0.2.15` line
  is corrected (address actually assigned, or message removed).

## Explicitly out of scope (documented, not omitted)

- **TCP echo session**: not drivable today — blocked on K10 (no lease → no
  reachable guest IP) AND on a guest-side TCP client/server existing (the
  kernel net stack is dhcp/dns/unix only; no listener; no userspace socket
  tool in the initrd). The probe's header audit row 10 records this so a
  future editor does not mistake it for a silent omission. The TCP-echo leg
  becomes a real stage once a lease exists and a socket tool ships.
