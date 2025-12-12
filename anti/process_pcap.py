from scapy.all import rdpcap, IP
import sys
import hashlib
import secrets

pkts = rdpcap(sys.argv[1])

SALT = secrets.token_bytes(32)

ref_time = pkts[0].time
for pkt in pkts:
    pkt.time -= ref_time


def chunks_exact(iterable, n):
    iterators = [iter(iterable)] * n
    return zip(*iterators)


ORDNS = '74.82.42.42'
pkts = [pkt for pkt in pkts if pkt.getlayer(IP).dst == ORDNS]


out = []
for ch_no, chunk in enumerate(chunks_exact(pkts, 25)):
    for pkt in chunk:
        src = pkt.getlayer(IP).src
        src_hash = hashlib.sha3_256(
            bytes(src, encoding='utf-8')
            + SALT
            + bytes(ch_no)
        ).hexdigest()
        dst = pkt.getlayer(IP).dst
        dst_hash = hashlib.sha3_256(
            bytes(dst, encoding='utf-8')
            + SALT
        ).hexdigest()
        domain = pkt.qd.qname
        domain_hash = hashlib.sha3_256(bytes(domain) + SALT).hexdigest()
        time = int(pkt.time*1000)
        json_line = f'{{"domain":"{domain_hash}","src":"{src_hash}","dst":"{dst_hash}","time":{time}}}'
        print(json_line)
