# Владение: 100000 записей в списке и на 1000 полках словаря, 300 шагов,
# каждый строит значение, ссылающееся на них (в Python регионов нет).
class Account:
    __slots__ = ("amount",)
    def __init__(self, amount): self.amount = amount
    def balance(self): return self.amount


class Vault:
    __slots__ = ("amount",)
    def __init__(self, amount): self.amount = amount
    def balance(self): return self.amount * 2


class Record:
    __slots__ = ("id", "ledger")
    def __init__(self, id, ledger): self.id, self.ledger = id, ledger
    def total(self): return self.id % 97 + self.ledger.balance()


class View:
    __slots__ = ("records", "shelves", "at")
    def __init__(self, records, shelves, at): self.records, self.shelves, self.at = records, shelves, at
    def total(self):
        shelf = self.shelves.get(self.at % len(self.shelves), self.records)
        return self.records[self.at].total() + shelf[self.at % len(shelf)].total()


class Store:
    def __init__(self, racks):
        records = [Record(i, Vault(i % 13) if i % 1000 == 0 else Account(i % 13)) for i in range(racks * 100)]
        self.records = records
        self.shelves = {shelf: records[shelf * 100:shelf * 100 + 100] for shelf in range(racks)}

    def sum(self, rounds):
        total = 0
        for round_ in range(rounds):
            fee = Vault(round_ % 5)
            view = View(self.records, self.shelves, round_ * 7919 % len(self.records))
            total += view.total() + fee.balance()
        return total



import sys, time
for _ in range(int(sys.argv[1])):
    t = time.perf_counter_ns()
    r = Store(1000).sum(300)
    print("iteration", time.perf_counter_ns() - t, file=sys.stderr)
print(r)
