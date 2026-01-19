# pub_xsub_and_sub_xpub.py
# Minimal: PUB -> XSUB sends a few messages; two SUB threads connect to XPUB and print.

import threading
import time
import zmq

XSUB_ENDPOINT = "tcp://127.0.0.1:5557"  # publishers connect here
XPUB_ENDPOINT = "tcp://127.0.0.1:5558"  # subscribers connect here
SUB_THREADS = 2

def sub_worker(name: str, ctx: zmq.Context) -> None:
    s = ctx.socket(zmq.SUB)
    s.connect(XPUB_ENDPOINT)
    s.setsockopt(zmq.SUBSCRIBE, b"")  # subscribe to everything
    while True:
        try:
            msg = s.recv_string()
            print(f"{name} <- {msg}")
        except zmq.ContextTerminated:
            break

def main() -> None:
    ctx = zmq.Context.instance()

    # Start a couple of subscriber threads (daemon so the process can exit).
    for i in range(SUB_THREADS):
        t = threading.Thread(target=sub_worker, args=(f"sub{i+1}", ctx), daemon=True)
        t.start()

    # Publisher: connect to XSUB and send a few messages.
    pub = ctx.socket(zmq.PUB)
    pub.connect(XSUB_ENDPOINT)

    # Allow time for subscriptions to propagate through XPUB/XSUB proxy.
    time.sleep(0.3)

    for i in range(3):
        pub.send_string(f"topic test {i}")
        print(f"pub -> topic test {i}")
        time.sleep(0.1)

    # Give subscribers a moment to receive, then clean up.
    time.sleep(0.5)
    ctx.term()

if __name__ == "__main__":
    main()
