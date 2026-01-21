# pub_xsub_and_sub_xpub.py
# Test: PUB -> XSUB proxy -> XPUB -> SUB
# Verifies the XSUB/XPUB proxy is forwarding messages correctly.

import threading
import time
import zmq

XSUB_ENDPOINT = "tcp://127.0.0.1:5557"  # publishers connect here
XPUB_ENDPOINT = "tcp://127.0.0.1:5558"  # subscribers connect here
NUM_SUBSCRIBERS = 2
NUM_MESSAGES = 3

# Track received messages per subscriber
received = {f"sub{i+1}": [] for i in range(NUM_SUBSCRIBERS)}
lock = threading.Lock()

def sub_worker(name: str, ctx: zmq.Context) -> None:
    s = ctx.socket(zmq.SUB)
    s.setsockopt(zmq.LINGER, 0)
    s.connect(XPUB_ENDPOINT)
    s.setsockopt(zmq.SUBSCRIBE, b"")  # subscribe to everything
    while True:
        try:
            msg = s.recv_string()
            with lock:
                received[name].append(msg)
        except zmq.ContextTerminated:
            break
    s.close()

def main() -> None:
    ctx = zmq.Context.instance()

    print(f"Starting {NUM_SUBSCRIBERS} subscribers...")
    for i in range(NUM_SUBSCRIBERS):
        t = threading.Thread(target=sub_worker, args=(f"sub{i+1}", ctx), daemon=True)
        t.start()

    # Publisher: connect to XSUB and send messages.
    pub = ctx.socket(zmq.PUB)
    pub.setsockopt(zmq.LINGER, 0)
    pub.connect(XSUB_ENDPOINT)

    # Allow time for subscriptions to propagate through XPUB/XSUB proxy.
    time.sleep(0.3)

    print(f"Publishing {NUM_MESSAGES} messages...")
    sent = []
    for i in range(NUM_MESSAGES):
        msg = f"message {i}"
        pub.send_string(msg)
        sent.append(msg)
        time.sleep(0.05)

    # Give subscribers time to receive.
    time.sleep(0.3)
    pub.close()

    # Print results
    print("\n" + "=" * 40)
    print("RESULTS")
    print("=" * 40)
    print(f"Sent: {sent}")

    all_ok = True
    for name, msgs in received.items():
        status = "OK" if msgs == sent else "FAIL"
        if status == "FAIL":
            all_ok = False
        print(f"{name}: {msgs} [{status}]")

    print("=" * 40)
    if all_ok:
        print("SUCCESS: All subscribers received all messages!")
    else:
        print("FAILURE: Some messages were not received.")
    print("=" * 40)

    ctx.term()

if __name__ == "__main__":
    main()
