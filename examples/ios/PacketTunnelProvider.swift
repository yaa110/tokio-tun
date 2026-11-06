import NetworkExtension
import Darwin

/// Minimal Packet Tunnel Provider example.
///
/// This uses only public API:
/// - `NEPacketTunnelFlow` reads/writes tunnel packets.
/// - A `SOCK_DGRAM` socket pair carries packets to and from Rust.
/// - The Rust end of the socket pair is wrapped in `tokio_tun::TunQueue`.
///
/// Do not try to extract a private fd via KVO or other non-public APIs.
final class PacketTunnelProvider: NEPacketTunnelProvider {
    private let lock = NSLock()
    private var tunnelFDs: [Int32] = [-1, -1]
    private var active = false
    private var readSource: DispatchSourceRead?
    private var readBuffer = [UInt8](repeating: 0, count: 65_535)

    override func startTunnel(
        options: [String: NSObject]?,
        completionHandler: @escaping (Error?) -> Void
    ) {
        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "10.0.0.1")
        let ipv4 = NEIPv4Settings(
            addresses: ["10.0.0.1"],
            subnetMasks: ["255.255.255.0"]
        )
        ipv4.includedRoutes = [NEIPv4Route.default()]
        settings.ipv4Settings = ipv4
        settings.mtu = 1500
        settings.dnsSettings = NEDNSSettings(servers: ["1.1.1.1"])

        setTunnelNetworkSettings(settings) { [weak self] error in
            guard error == nil else {
                completionHandler(error)
                return
            }
            do {
                try self?.startBridge()
                completionHandler(nil)
            } catch {
                completionHandler(error)
            }
        }
    }

    private func startBridge() throws {
        guard !isActive() else { return }

        var fds: [Int32] = [-1, -1]
        guard socketpair(AF_UNIX, SOCK_DGRAM, 0, &fds) == 0 else {
            throw posixError()
        }

        // Keep the Swift end non-blocking so a slow Rust consumer cannot stall
        // the Network Extension packet-flow callback.
        let flags = fcntl(fds[0], F_GETFL, 0)
        _ = fcntl(fds[0], F_SETFL, flags | O_NONBLOCK)

        // Ownership of fds[1] is transferred to Rust even if startup fails;
        // Rust closes it on every failure path. fds[0] remains owned by Swift.
        guard tokio_tun_start(fds[1]) == 0 else {
            close(fds[0])
            throw NSError(
                domain: "TokioTun",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "Rust tunnel start failed"]
            )
        }

        lock.lock()
        tunnelFDs = [fds[0], -1]
        active = true
        lock.unlock()

        let source = DispatchSource.makeReadSource(
            fileDescriptor: fds[0],
            queue: .global()
        )
        source.setEventHandler { [weak self] in
            self?.drainRustQueue()
        }
        source.resume()
        readSource = source

        readFromPacketFlow()
    }

    /// Packets entering the tunnel are forwarded to Rust through fds[0].
    private func readFromPacketFlow() {
        guard isActive() else { return }
        packetFlow.readPackets { [weak self] packets, _ in
            guard let self, self.isActive() else { return }
            for packet in packets {
                self.writeToRust(packet)
            }
            self.readFromPacketFlow()
        }
    }

    private func writeToRust(_ packet: Data) {
        withSwiftFD { fd in
            packet.withUnsafeBytes { buffer in
                guard let base = buffer.baseAddress else { return }
                let written = write(fd, base, buffer.count)
                if written < 0 && errno != EAGAIN && errno != EWOULDBLOCK {
                    NSLog("tokio-tun: socket write failed: %d", errno)
                }
            }
        }
    }

    /// Packets produced by Rust are injected back into the tunnel.
    private func drainRustQueue() {
        withSwiftFD { fd in
            let length = readBuffer.withUnsafeMutableBytes { buffer in
                read(fd, buffer.baseAddress, buffer.count)
            }
            guard length > 0 else { return }

            let data = Data(readBuffer[0..<length])
            let version = readBuffer[0] >> 4
            let protocolNumber = version == 4 ? AF_INET : AF_INET6
            packetFlow.writePackets(
                [data],
                withProtocols: [NSNumber(value: protocolNumber)]
            )
        }
    }

    override func stopTunnel(
        with reason: NEProviderStopReason,
        completionHandler: @escaping () -> Void
    ) {
        lock.lock()
        active = false
        let fd = tunnelFDs[0]
        tunnelFDs = [-1, -1]
        lock.unlock()

        readSource?.cancel()
        readSource = nil
        tokio_tun_stop()
        if fd >= 0 {
            close(fd)
        }
        completionHandler()
    }

    private func isActive() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return active
    }

    private func withSwiftFD(_ body: (Int32) -> Void) {
        lock.lock()
        defer { lock.unlock() }
        guard active, tunnelFDs[0] >= 0 else { return }
        body(tunnelFDs[0])
    }

    private func posixError() -> NSError {
        NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
    }
}
