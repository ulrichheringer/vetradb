#!/usr/bin/env python3
"""Independent checks of foundation examples; not a storage/recovery implementation."""
import json
import struct
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / 'docs/fixtures/foundation'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def crc32c(data):
    value = 0xffffffff
    for byte in data:
        value ^= byte
        for _ in range(8):
            value = (value >> 1) ^ (0x82f63b78 if value & 1 else 0)
    return value ^ 0xffffffff


def checksum(data, offset):
    require(len(data) >= offset + 4, 'truncated checksum')
    expected = struct.unpack_from('<I', data, offset)[0]
    cleared = data[:offset] + bytes(4) + data[offset + 4:]
    require(crc32c(cleared) == expected, 'checksum mismatch')


def fixture(name):
    return bytes.fromhex((FIXTURES / (name + '.hex')).read_text())


def superblock(data):
    require(len(data) == 8192, 'superblock length')
    require(data[:8] == b'VETRASB1', 'superblock magic')
    require(struct.unpack_from('<HHI', data, 8) == (1, 128, 8192), 'superblock version/size')
    checksum(data, 116)
    require(any(data[16:32]) and any(data[32:48]), 'empty lineage')
    require(struct.unpack_from('<Q', data, 48)[0] != 0, 'generation')
    require(not any(data[112:116] + data[120:]), 'superblock reserved')
    for offset in (72, 88):
        page, gen = struct.unpack_from('<QQ', data, offset)
        require((page == gen == 0) or (page >= 2 and gen > 0), 'root address')


def leaf(data):
    require(len(data) == 8192 and data[:4] == b'VPG1', 'page length/magic')
    require(struct.unpack_from('<HH', data, 4) == (1, 1), 'leaf version/kind')
    checksum(data, 48)
    page, gen, _, tree = struct.unpack_from('<QQQQ', data, 8)
    require(page >= 2 and gen > 0 and tree > 0, 'page identity')
    lower, upper, count, flags = struct.unpack_from('<HHHH', data, 40)
    require(flags == 0 and not any(data[52:64] + data[116:128]), 'page reserved')
    require(lower == 128 + 4 * count and lower <= upper <= 8192, 'slot bounds')
    for offset in (64, 80):
        sibling, generation = struct.unpack_from('<QQ', data, offset)
        require((sibling == generation == 0) or (sibling >= 2 and generation > 0), 'sibling')
    require(not any(data[96:114]), 'leaf child/level')
    high = struct.unpack_from('<H', data, 114)[0]
    require(high == 65535 or high < count, 'high key')
    spans = []
    for i in range(count):
        start, length = struct.unpack_from('<HH', data, 128 + 4 * i)
        require(start >= upper and length >= 32 and start + length <= 8192, 'slot range')
        spans.append((start, start + length))
    spans.sort()
    require(all(a[1] <= b[0] for a, b in zip(spans, spans[1:])), 'overlapping slots')


def image(data):
    require(len(data) >= 4, 'image header')
    count, reserved = struct.unpack_from('<HH', data)
    require(count <= 4096 and reserved == 0, 'image count/reserved')
    pos, previous = 4, 0
    for _ in range(count):
        require(pos + 16 <= len(data), 'field header')
        field, tag, reserved, length = struct.unpack_from('<QB3sI', data, pos)
        pos += 16
        require(field > previous and not any(reserved) and tag <= 5, 'field identity/tag')
        require(pos + length <= len(data) and length <= 16777216, 'field length')
        value = data[pos:pos + length]
        if tag == 0:
            require(length == 0, 'null length')
        elif tag == 2:
            value.decode('utf-8')
        elif tag in (3, 4):
            require(length == 8, 'integer length')
        elif tag == 5:
            require(length == 1 and value[0] <= 1, 'boolean')
        pos += length
        previous = field
    require(pos == len(data), 'image trailing bytes')


def envelope(data):
    require(96 <= len(data) <= 16777216 and data[:8] == b'VENV0001', 'envelope length/magic')
    require(struct.unpack_from('<HHI', data, 8) == (1, 96, len(data)), 'envelope version/size')
    checksum(data, 88)
    require(any(data[16:32]) and any(data[32:48]), 'envelope lineage')
    tx, csn = struct.unpack_from('<QQ', data, 48)
    count, meta_len, principal = struct.unpack_from('<IIQ', data, 72)
    require(tx > 0 and csn > 0 and principal > 0, 'envelope identities')
    require(count <= 65535 and 4 <= meta_len <= 16384 and 96 + meta_len <= len(data), 'metadata bounds')
    require(not any(data[92:96]), 'envelope flags')
    meta = data[96:96 + meta_len]
    entries, reserved = struct.unpack_from('<HH', meta)
    require(entries <= 64 and reserved == 0, 'metadata count')
    pos, previous = 4, b''
    for _ in range(entries):
        require(pos + 4 <= len(meta), 'metadata header')
        key_len, value_len = struct.unpack_from('<HH', meta, pos)
        pos += 4
        require(1 <= key_len <= 128 and value_len <= 4096 and pos + key_len + value_len <= len(meta), 'metadata entry')
        key = meta[pos:pos + key_len]
        key.decode('utf-8')
        meta[pos + key_len:pos + key_len + value_len].decode('utf-8')
        require(key > previous and not key.startswith(b'vetra.'), 'metadata ownership/order')
        previous = key
        pos += key_len + value_len
    require(pos == len(meta), 'metadata trailing bytes')
    pos = 96 + meta_len
    for index in range(count):
        require(pos + 32 <= len(data), 'operation header')
        order, participant, action, codec, obj, schema, key_len, payload_len = struct.unpack_from('<IBBHQQII', data, pos)
        pos += 32
        require(order == index and 1 <= participant <= 7 and action in (1, 2) and codec == 1 and obj > 0, 'operation identity')
        require((schema > 0) if participant in (1, 2) else (schema == 0), 'operation schema')
        require(1 <= key_len <= 8192 and payload_len >= 8 and pos + key_len + payload_len <= len(data), 'operation bounds')
        pos += key_len
        payload = data[pos:pos + payload_len]
        before, after = struct.unpack_from('<II', payload)
        require(8 + before + after == len(payload), 'image lengths')
        require(after > 0 if action == 1 else (before > 0 and after == 0), 'action images')
        if before:
            image(payload[8:8 + before])
        if after:
            image(payload[8 + before:])
        pos += payload_len
    require(pos == len(data), 'envelope trailing bytes')
    return tx, csn


def wal(data, lsn):
    require(64 <= len(data) <= 1048576 and data[:4] == b'VWR1', 'WAL length/magic')
    total, version, kind, flags = struct.unpack_from('<IHHI', data, 4)
    actual, tx, prev, page, gen = struct.unpack_from('<QQQQQ', data, 16)
    length = struct.unpack_from('<I', data, 56)[0]
    require(total == len(data) == 64 + length and version == 1 and 1 <= kind <= 14 and flags == 0, 'WAL header')
    require(actual == lsn and lsn % 67108864 >= 64 and lsn % 67108864 + total <= 67108864, 'WAL position')
    require(prev == 0 or prev < lsn, 'WAL backward link')
    require((page == gen == 0) or (page >= 2 and gen > 0), 'WAL page address')
    checksum(data, 60)
    return kind, tx, data[64:]


def graph_check(graph):
    require(all(dep in graph for deps in graph.values() for dep in deps), 'unknown module')
    done, active = set(), set()

    def visit(node):
        require(node not in active, 'dependency cycle')
        if node in done:
            return
        active.add(node)
        for dep in graph[node]:
            visit(dep)
        active.remove(node)
        done.add(node)

    for node in graph:
        visit(node)
    closure = set()

    def collect(node):
        if node not in closure:
            closure.add(node)
            for dep in graph[node]:
                collect(dep)
    collect('embedded')
    require(not closure.intersection({'server', 'pgwire', 'cli'}), 'embedded adapter coupling')


def altered(data, offset, value, crc_offset):
    copy = bytearray(data)
    copy[offset:offset + len(value)] = value
    copy[crc_offset:crc_offset + 4] = bytes(4)
    struct.pack_into('<I', copy, crc_offset, crc32c(copy))
    return bytes(copy)


class FoundationExamples(unittest.TestCase):
    def test_checksum_known_vector(self):
        self.assertEqual(crc32c(b'123456789'), 0xe3069283)

    def test_superblock(self):
        superblock(fixture('superblock'))

    def test_leaf(self):
        leaf(fixture('empty-leaf'))

    def test_envelope(self):
        self.assertEqual(envelope(fixture('row-envelope')), (7, 9))

    def test_segment(self):
        data = fixture('segment')
        self.assertEqual(len(data), 64)
        self.assertEqual(data[:8], b'VETRAWL1')
        self.assertEqual(struct.unpack_from('<HHI', data, 8), (1, 64, 67108864))
        checksum(data, 56)
        self.assertFalse(any(data[60:64]))

    def test_chunk_commit_agreement(self):
        chunk = fixture('envelope-chunk')
        kind, tx, payload = wal(chunk, 64)
        self.assertEqual((kind, tx), (8, 7))
        index, count, total, length = struct.unpack_from('<IIII', payload)
        self.assertEqual((index, count, total, length), (0, 1, len(payload) - 16, len(payload) - 16))
        env = payload[16:]
        self.assertEqual(env, fixture('row-envelope'))
        self.assertEqual(envelope(env), (7, 9))
        kind, tx, commit = wal(fixture('commit'), 64 + len(chunk))
        self.assertEqual((kind, tx), (9, 7))
        self.assertEqual(struct.unpack('<QIIQ', commit), (9, len(env), struct.unpack_from('<I', env, 88)[0], 64))

    def test_truncation_and_bit_corruption(self):
        for name, check in [('superblock', superblock), ('empty-leaf', leaf), ('row-envelope', envelope)]:
            data = fixture(name)
            with self.subTest(name=name):
                with self.assertRaises(ValueError):
                    check(data[:-1])
                damaged = bytearray(data)
                damaged[-1] ^= 1
                with self.assertRaises(ValueError):
                    check(damaged)

    def test_unknown_versions_with_valid_checksums(self):
        for name, check, crc in [('superblock', superblock, 116), ('empty-leaf', leaf, 48), ('row-envelope', envelope, 88)]:
            offset = 4 if name == 'empty-leaf' else 8
            with self.assertRaises(ValueError):
                check(altered(fixture(name), offset, b'\x02\x00', crc))

    def test_invalid_slot_bounds(self):
        with self.assertRaises(ValueError):
            leaf(altered(fixture('empty-leaf'), 40, struct.pack('<H', 8193), 48))

    def test_noncontiguous_operation(self):
        with self.assertRaises(ValueError):
            envelope(altered(fixture('row-envelope'), 100, struct.pack('<I', 3), 88))

    def test_oversized_metadata(self):
        with self.assertRaises(ValueError):
            envelope(altered(fixture('row-envelope'), 76, struct.pack('<I', 16385), 88))

    def test_unknown_participant(self):
        with self.assertRaises(ValueError):
            envelope(altered(fixture('row-envelope'), 104, b'\x08', 88))

    def test_wal_bad_position_and_torn_tail(self):
        data = fixture('commit')
        with self.assertRaises(ValueError):
            wal(data, 1)
        with self.assertRaises(ValueError):
            wal(data[:-1], 64 + len(fixture('envelope-chunk')))

    def test_module_graph(self):
        graph_check(json.loads((ROOT / 'docs/specs/modules.json').read_text()))

    def test_cycle_or_hidden_adapter_is_rejected(self):
        graph = json.loads((ROOT / 'docs/specs/modules.json').read_text())
        graph['txn'].append('catalog')
        with self.assertRaises(ValueError):
            graph_check(graph)
        graph['txn'].remove('catalog')
        graph['embedded'].append('server')
        with self.assertRaises(ValueError):
            graph_check(graph)


if __name__ == '__main__':
    unittest.main(verbosity=2)
