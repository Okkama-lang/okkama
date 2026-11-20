"""Python counterparts of bench/micro programs (same work, same checksums), protocol micro-v2."""
import sys, time

class Point:
    __slots__ = ("x", "y")
    def __init__(self, x, y): self.x, self.y = x, y
    def sum(self): return self.x + self.y
    def moved(self, dx): return Point(self.x + dx, self.y)

class Res:
    __slots__ = ("id",)
    def __init__(self, id): self.id = id
    def __enter__(self): return self
    def __exit__(self, *exc): return False

class Maker:
    def make(self, i):
        with Res(i) as r:
            return r.id

class Resource:
    __slots__ = ("count",)
    def __init__(self, count): self.count = count
    def __enter__(self): return self
    def __exit__(self, *exc):
        self.count[0] += 1
        return False

class Sink:
    def touch(self): pass

class Deferring:
    def value(self, i):
        try:
            return i % 7
        finally:
            Sink().touch()

def baseline(n):
    acc = 0
    for i in range(n): acc += i % 7
    return acc

def constructor(n):
    acc = 0
    for i in range(n): acc += Point(i, 1).sum() % 7
    return acc

def successor(n):
    acc = 0
    for i in range(n): acc += Point(i, 1).moved(2).sum() % 7
    return acc

def destroy(n):
    acc = 0
    for i in range(n): acc += Maker().make(i) % 7
    return acc

def destroy_observed(n):
    count = [0]
    acc = 0
    for i in range(n):
        with Resource(count):
            value = i % 7
        acc += value
    return acc + count[0]

def defer(n):
    acc = 0
    for i in range(n): acc += Deferring().value(i)
    return acc

CASES = {"baseline": baseline, "constructor": constructor, "successor": successor, "destroy": destroy,
         "destroy-observed": destroy_observed, "defer": defer}

name, n, warm = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
f = CASES[name]
w = f(warm)
t = time.perf_counter_ns()
c = f(n)
print("micro-v2", n, warm, time.perf_counter_ns() - t, c, w)
