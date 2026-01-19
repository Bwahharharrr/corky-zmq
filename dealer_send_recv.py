# dealer_send_recv.py
# Minimal: connect DEALER -> tcp://127.0.0.1:5560, send one message, try to receive a reply.

import time
import zmq

DEALER_ENDPOINT = "tcp://127.0.0.1:5560"  # broker's worker_facing_dealer
IDENTITY = b"py-dealer"
RECV_TIMEOUT_MS = 3000  # 3s

def main() -> None:
    ctx = zmq.Context.instance()
    sock = ctx.socket(zmq.DEALER)
    sock.setsockopt(zmq.IDENTITY, IDENTITY)
    sock.connect(DEALER_ENDPOINT)

    sock.send_multipart([b"hello from python dealer"])
    print(f"sent -> {DEALER_ENDPOINT}")

    poller = zmq.Poller()
    poller.register(sock, zmq.POLLIN)
    events = dict(poller.poll(RECV_TIMEOUT_MS))
    if sock in events:
        msg = sock.recv_multipart()
        print("recv <-", msg)
    else:
        print("no reply (timeout)")

    sock.close(0)
    ctx.term()

if __name__ == "__main__":
    main()
