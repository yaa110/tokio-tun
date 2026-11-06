# Android example

This example shows the minimal public-API path for using `tokio-tun` on
Android. Android does not allow apps to open `/dev/net/tun`; the system
provides a TUN descriptor through `VpnService`.

## Flow

1. An Activity obtains user consent with `VpnService.prepare()`.
2. `TokioTunService` calls `VpnService.Builder.establish()`.
3. The Kotlin side calls `ParcelFileDescriptor.detachFd()`; ownership of that
   raw descriptor moves to native code.
4. JNI passes the descriptor to Rust, where `TunQueue::try_from` wraps it.
5. Rust reads raw IPv4/IPv6 packets through `TunQueue`; the example stub leaves
   the outgoing packet-processing path to your transport.

`Tun` and `TunBuilder` are intentionally unavailable on Android. Configure
addresses, routes, MTU, DNS, and allowed applications through
`VpnService.Builder`.

## Files

- [`VpnService.kt`](VpnService.kt): service and fd transfer.
- [`tun_bridge.rs`](tun_bridge.rs): illustrative Rust/JNI bridge.

## Manifest

Declare the service and VPN permission in the app manifest:

```xml
<service
    android:name=".TokioTunService"
    android:exported="false"
    android:permission="android.permission.BIND_VPN_SERVICE">
    <intent-filter>
        <action android:name="android.net.VpnService" />
    </intent-filter>
</service>
```

Start the service only after consent:

```kotlin
val intent = VpnService.prepare(context)
if (intent != null) {
    // Launch an Activity for the consent intent.
} else {
    context.startService(Intent(context, TokioTunService::class.java))
}
```

## Rust/JNI notes

- Compile the Rust bridge as a `cdylib` and load it with
  `System.loadLibrary("tokio_tun_bridge")`.
- Call `adopt_fd` from inside a Tokio runtime with I/O enabled.
- `TunQueue::new`/`try_from` sets the descriptor non-blocking. It leaves
  `FD_CLOEXEC` unchanged; set it if the process may `exec`.
- Each `recv` returns one packet. Use a receive buffer at least as large as the
  MTU; raw reads can silently truncate larger packets.
- `VpnService.Builder` owns interface configuration, not `tokio-tun`.
- Use `VpnService.protect()` for sockets that must bypass the tunnel.
