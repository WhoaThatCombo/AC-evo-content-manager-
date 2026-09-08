"""How many protobuf messages are actually distinguishable from a loader?

A loader only reveals: (a) which offsets it reads, (b) that they are floats.
So the observable signature of a message is its float/non-float pattern by
index. Count how many messages have a UNIQUE signature - those can be named
with certainty; the rest need call-site types.
"""
import sys
from collections import Counter, defaultdict
sys.path.insert(0, r'C:\Projects\acecm')
from acecm import protos
from google.protobuf.descriptor import FieldDescriptor as FD

FOUR = {FD.TYPE_FLOAT, FD.TYPE_INT32, FD.TYPE_UINT32, FD.TYPE_ENUM,
        FD.TYPE_SINT32, FD.TYPE_FIXED32, FD.TYPE_SFIXED32}


def repeated(f):
    try:
        return f.is_repeated
    except AttributeError:
        return False


msgs = {}
for n in protos.message_names():
    try:
        m = protos.new(n)
    except Exception:
        continue
    fs = list(m.DESCRIPTOR.fields)
    if not fs or any(f.message_type is not None or repeated(f) or
                     f.type not in FOUR for f in fs):
        continue
    fs.sort(key=lambda f: f.number)
    msgs[n] = tuple(f.type == FD.TYPE_FLOAT for f in fs)

print('all-4-byte-scalar messages: %d' % len(msgs))

# signature 1: full float/non-float pattern (what a loader can observe)
sig = defaultdict(list)
for n, pat in msgs.items():
    sig[pat].append(n)
uniq = [v[0] for v in sig.values() if len(v) == 1]
print()
print('=== signature = float/non-float pattern by index ===')
print('distinct signatures      : %d' % len(sig))
print('UNIQUELY identifiable    : %d  (%.1f%%)'
      % (len(uniq), 100.0 * len(uniq) / len(msgs)))
print('in a colliding group     : %d' % (len(msgs) - len(uniq)))
big = sorted(sig.values(), key=len, reverse=True)[:6]
print('largest collision groups :')
for g in big:
    if len(g) == 1:
        continue
    pat = msgs[g[0]]
    print('   %d fields, %d floats -> %d messages: %s'
          % (len(pat), sum(pat), len(g), ', '.join(sorted(g)[:5])
             + (' ...' if len(g) > 5 else '')))

# how bad are the all-float ones (no int positions to disambiguate)?
allfloat = {n: p for n, p in msgs.items() if all(p)}
print()
print('all-float messages (no int positions at all): %d' % len(allfloat))
cnt = Counter(len(p) for p in allfloat.values())
print('   by field count -> how many messages share that count:')
for k in sorted(cnt):
    if cnt[k] > 1:
        print('      %2d fields : %d messages  <- mutually indistinguishable'
              % (k, cnt[k]))
solo = sum(1 for k in cnt if cnt[k] == 1)
print('   all-float messages with a unique field count: %d' % solo)

# practical: a loader rarely reads every field. If it reads k of n floats,
# how much worse does it get? Report messages whose float-count is unique.
fc = Counter(sum(p) for p in msgs.values())
uniq_fc = [n for n, p in msgs.items() if fc[sum(p)] == 1]
print()
print('messages whose FLOAT COUNT alone is unique: %d' % len(uniq_fc))
