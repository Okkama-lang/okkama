# Okkama style guide (draft 1)

How Okkama code is written: what is allowed, what is not, and why. Draft 1
(2026-11-28, ticket #1189), not yet reviewed by the language's owner; the
rules may still change. Read `okkama agents language` first: this guide
assumes the syntax.

**How to read it.** Every rule has a reason, a bad example (**Avoid**) and a
good one (**Prefer**). Each rule is marked by who enforces it:

- **compiler**: `okkama check` rejects the bad form. The `okkama,fail`
  block shows the exact diagnostic. You cannot break these rules.
- **lint**: `okkama lint` reports it (section 12 lists what lint checks
  today; no style rule below is a lint finding yet).
- **review**: judgement. Nothing stops the bad form; this guide and the
  reviewer do.

Every example is compiled by `docs/agents/check.py` in CI, and the ones
that show output are run. The good examples are also formatted
(`okkama fmt --check`) and lint-clean (`okkama lint`), so they carry the
doc comments lint asks for. The bad ones start with a `// Avoid:` comment.
Section 13 is the whole guide as a one-screen checklist.

**Where this comes from.** The rules are the principles the standard
library is written by, and the Okkama Bible's chapters on objects, roles,
tests, async and patterns (12–15, 20, 25), applied to application code. The spirit is that
of *Elegant Objects* (Yegor Bugayenko, 2016–2017): objects are living
mechanisms, not data with procedures around them; no getters, no setters,
no mutation, small objects, decorators. Okkama's compiler already enforces
much of that, so this guide spends its words on the rest. It is milder
than Elegant Objects: records at the boundaries, hot loops and interop are
listed exceptions (section 11), an enum with a `match` is fine for a closed
set, and an accessor is a last resort rather than a crime. The form
(short rules, the reason first, a checklist) follows the Rust API
Guidelines and Go's Code Review Comments.

## Contents

1. Encapsulation
2. Immutability
3. Small objects
4. Roles and composition
5. The shape of an API
6. Errors
7. Async
8. Resources
9. Tests
10. Anti-patterns
11. Pragmatic exceptions
12. What the tools check
13. Checklist

## 1. Encapsulation

An object is a box that does something and gives a result. It never shows
its state; callers talk to it only through what it does.

### 1.1 Fields are private: ask the object (compiler)

**Why.** Code that reads an object's fields and computes from them belongs
inside the object. Okkama makes fields private to the object, not even
visible to the rest of its module.

```okkama,fail
module fields;

object Price {
    has cents is int;

    Price(:cents);
}

main() is int {
    Price(cents: 250).cents;
}
```
```error
example.ok:10:23: semantic error: Field "cents" of object "Price" is private; call an action instead
```

### 1.2 Tell, don't ask: no getters of internals (review)

**Why.** A getter turns the object into a record and moves its logic into
every caller; the next caller repeats it, or gets it wrong. An action whose
body only returns a field (`cents() is int { cents; }`) is an accessor.
Before writing one, ask what the caller wants to do with the value, and give
the object that action instead.

**Avoid**

```okkama
module invoice_getters;

// Avoid: the invoice hands out its parts, and every caller does its arithmetic.
object Line {
    has price, count is int;

    Line(:price, :count);

    price() is int {
        price;
    }

    count() is int {
        count;
    }
}

object Invoice {
    has lines is @Line;

    Invoice(:lines);

    lines() is @Line {
        lines;
    }
}

main() is int {
    let invoice = Invoice(lines: [Line(price: 250, count: 2), Line(price: 100, count: 1)]);
    let total = invoice.lines().reduce(0) -> sum, line {
        sum + line.price() * line.count();
    };
    0;
}
```

**Prefer**

```okkama,run
module invoice;

import "io/stdout" take Stdout;

// One line of an invoice.
object Line {
    has price, count is int;

    // A line of count items at price cents each.
    Line(:price, :count);

    // What the line costs, in cents.
    cost() is int {
        price * count;
    }
}

// Lines that add up to an amount due.
object Invoice {
    has lines is @Line;

    // An invoice of lines.
    Invoice(:lines);

    // The amount due, in cents.
    total() is int {
        lines.reduce(0) -> sum, line {
            sum + line.cost();
        };
    }
}

main() is int {
    let invoice = Invoice(lines: [Line(price: 250, count: 2), Line(price: 100, count: 1)]);
    Stdout().writeln(invoice.total().string());
    0;
}
```
```output
600
```

### 1.3 The object answers in the form you use (review)

**Why.** Every object knows what information it gives and in what form.
That knowledge goes inside its action, not into fences of decoders,
formatters and converters that each caller must assemble around it. std
does this everywhere: `message.string()`, `date.days_until(other)`,
`process.output().text(limit:)`. A formatter object stays for the rare case
where the format really is the caller's choice, and then the object's own
action uses it inside.

**Avoid**

```okkama
module temperature_fence;

// Avoid: a formatter fence; every caller must know the unit and the format.
object Temperature {
    has kelvin is int;

    Temperature(:kelvin);

    kelvin() is int {
        kelvin;
    }
}

object CelsiusFormatter {
    CelsiusFormatter();

    text(kelvin is int) is string {
        (kelvin - 273).string() + " C";
    }
}

main() is int {
    let shown = CelsiusFormatter().text(Temperature(kelvin: 294).kelvin());
    0;
}
```

**Prefer**

```okkama,run
module temperature;

import "io/stdout" take Stdout;

// A temperature, kept in kelvins.
object Temperature {
    has kelvin is int;

    // A temperature of kelvin kelvins.
    Temperature(:kelvin);

    // The temperature as people read it: "21 C".
    celsius() is string {
        (kelvin - 273).string() + " C";
    }

    // The temperature in kelvins: "294 K".
    string() is string {
        kelvin.string() + " K";
    }
}

main() is int {
    let today = Temperature(kelvin: 294);
    Stdout().writeln(today.celsius(), ", ", today);
    0;
}
```
```output
21 C, 294 K
```

An object with `string()` goes straight into `writeln` and every other
`...string` parameter: text is the most common form, so give it.

### 1.4 Hide what callers do not need: `hidden` (compiler)

**Why.** The public surface is a promise. Mark every helper action
`hidden`, one by one, so the public surface is exactly the unmarked
actions. To hide a whole implementation, make the role public, the object
`hidden`, and return the role from a public action: callers can neither
name the object nor reach its other actions.

```okkama,fail
module hiding;

object Tab {
    has total is int = 0;

    Tab();

    hidden doubled() is int {
        total * 2;
    }
}

main() is int {
    Tab().doubled();
}
```
```error
example.ok:14:11: semantic error: Action "doubled" on object "Tab" is hidden
```

**Prefer**

```okkama,run
module greetings;

import "io/stdout" take Stdout;

// Something that greets a person.
role Greeting {
    // The greeting for name.
    text(name is string) is string;
}

hidden object Formal does Greeting {
    has title is string;

    Formal(:title);

    text(name is string) is string {
        "Good day, " + self.addressed(name);
    }

    hidden addressed(name is string) is string {
        title + " " + name;
    }
}

// Where greetings come from.
object Greetings {
    // The greetings.
    Greetings();

    // A formal greeting: the title before the name.
    formal(title is string) is Greeting {
        Formal(title: title);
    }
}

main() is int {
    Stdout().writeln(Greetings().formal("Dr.").text("Ada"));
    0;
}
```
```output
Good day, Dr. Ada
```

### 1.5 When callers must see parts, grant a cap (review)

**Why.** Sometimes a part really is the answer: a date's year, a URL's
host, a status code. Then the object grants a *cap*, a named read-only view
of computed scalar fields, instead of a getter per field. A cap holds what
is already computed or trivially computable, does no work and calls no
other object; anything that needs work stays an action. Prefer an action
when the caller's real question can be answered by the object (1.2).

Fields are private per instance: inside `order(other is Money)` the code
cannot read `other.amount` either. Comparing two instances is the usual
reason for a cap: `order()` reads `other::Amount::cents`, as std's calendar
dates do (`other::Calendar::year`), and no getter appears.

**Avoid**

```okkama
module date_getters;

// Avoid: a getter per field; the object is a record with a constructor.
object Day {
    has year, month, day is int;

    Day(:year, :month, :day);

    year() is int {
        year;
    }

    month() is int {
        month;
    }

    day() is int {
        day;
    }
}
```

**Prefer**

```okkama,run
module dates;

import "io/stdout" take Stdout;

// A day of the calendar.
object Day {
    has year, month, day is int;

    // The day of a month of a year.
    Day(:year, :month, :day);

    // The calendar parts of the day.
    cap Calendar {
        // The year.
        year is int {
            year;
        }

        // The month, 1 to 12.
        month is int {
            month;
        }
    }
}

main() is int {
    let calendar = Day(year: 2026, month: 11, day: 28)::Calendar;
    Stdout().writeln(calendar::year.string(), " ", calendar::month.string());
    0;
}
```
```output
2026 11
```

## 2. Immutability

Values never change. A change is a new value, the *successor*; the old one
stays as it was and may still be used.

### 2.1 A change is a successor (compiler)

**Why.** An object that cannot change can be shared, stored, compared and
handed to another task without fear, and each earlier state is its own
snapshot. A field is bound once, by the constructor. An action that
changes state is declared `is ^Self`, stages new fields with `^field = …`,
and returns the new object.

```okkama,fail
module mutation;

object Stock {
    has count is int = 0;

    Stock();

    received(amount is int) is void {
        count = count + amount;
    }
}
```
```error
example.ok:9:9: semantic error: Field "count" cannot be assigned after construction: objects are immutable; return a successor instead (`is ^Stock` and `^count = …`)
```

**Prefer**

```okkama,run
module stock;

import "io/stdout" take Stdout;

// Items of one kind in a warehouse.
object Stock {
    has count is int = 0;

    // An empty stock.
    Stock();

    // The stock after amount more arrived.
    received(amount is int) is ^Stock {
        ^count = count + amount;
    }

    // The stock after amount left; refused when there are not enough.
    shipped(amount is int) is ^Stock {
        confirm amount <= count else throw StockError.short(count, amount);
        ^count = count - amount;
    }

    // How the stock reads on a shelf label.
    string() is string {
        count.string() + " in stock";
    }
}

// Why a stock refused a shipment.
//
// - `short`: fewer items are left than were asked for.
enum StockError is Error = (short(have is int, asked is int));

main() is int {
    let empty = Stock();
    let stock = empty.received(10).shipped(3);
    Stdout().writeln(stock, "; before: ", empty);
    0;
}
```
```output
7 in stock; before: 0 in stock
```

### 2.2 Name the event, not the field (compiler, review)

**Why.** A successor that replaces one field (`set_total`, `with_total`) is
a setter in disguise: the caller does the thinking and the object only
stores it. Name the successor after what happened (`deposited`, `shipped`,
`joined`) and let it do the operation the change was for. The compiler
rejects `with` and `with_…` outright; `set_…` compiles, but is the same
mistake.

```okkama,fail
module wither;

object Account {
    has balance is int = 0;

    Account();

    with_balance(balance is int) is ^Account { ^balance = balance; }
}
```
```error
example.ok:8:5: syntax error at "with_balance": Action names "with" and "with_*" are forbidden; use domain composition or an explicitly named result.
  |
8 |     with_balance(balance is int) is ^Account { ^balance = balance; }
  |     ^^^^^^^^^^^^
```

**Avoid**

```okkama
module setter;

// Avoid: a setter; the caller computes the new balance.
object Account {
    has balance is int = 0;

    Account();

    set_balance(amount is int) is ^Account {
        ^balance = amount;
    }

    balance() is int {
        balance;
    }
}

main() is int {
    let account = Account();
    account.set_balance(account.balance() + 50).balance();
}
```

**Prefer**

```okkama,run
module deposits;

import "io/stdout" take Stdout;

// A bank account.
object Account {
    has balance is int = 0;

    // An empty account.
    Account();

    // The account after amount was paid in.
    deposited(amount is int) is ^Account {
        ^balance = balance + amount;
    }

    // The balance as a statement shows it: "50 EUR".
    string() is string {
        balance.string() + " EUR";
    }
}

main() is int {
    Stdout().writeln(Account().deposited(50));
    0;
}
```
```output
50 EUR
```

### 2.3 Accumulate with `reduce`, then a successor, then `Cell` (compiler, review)

**Why.** A running value is the classic reason to mutate. In Okkama, prefer
`reduce`, which says what is accumulated in one expression; then a
successor rebound in a loop (`tab = tab.ordered(price);`); then `Cell`, the
one mutable box of synchronous code. A `Cell` is local by design: the
compiler refuses it as a field and in async code, so it cannot become an
object's hidden state.

**Avoid**

```okkama
module cell_total;

import "cell" take Cell;

// Avoid: a mutable box where a reduce says the same thing.
object Words {
    has items is @string;

    Words(:items);

    letters() is int {
        let count = Cell(0);
        for items -> word {
            count.replace(count.value() + word.length());
        }
        count.value();
    }
}
```

**Prefer**

```okkama
module words;

// A list of words.
object Words {
    has items is @string;

    // The words items.
    Words(:items);

    // How many letters the words have together.
    letters() is int {
        items.reduce(0) -> count, word {
            count + word.length();
        };
    }
}
```

Shared mutable state between objects or tasks is an `Atomic` or a task that
owns the state (7.4).

## 3. Small objects

### 3.1 One responsibility, named for what it does (review)

**Why.** An object that does one thing is easy to name, test and replace.
A `Manager`, `Helper`, `Util`, `Processor` or `Handler` does everything
nobody else wanted: its name says nothing, so its scope never stops
growing. Name an object for what it *is* or does (`Invoice`, `Sales`,
`Receipt`), and wire the small objects together in `main` or in the object
that needs them.

**Avoid**

```okkama
module sales_manager;

import "io/stdout" take Stdout;

// Avoid: one object computes, judges and prints; its name says nothing.
object SalesManager {
    SalesManager();

    report(amounts is @int) is void {
        let total = amounts.reduce(0) -> sum, amount {
            sum + amount;
        };
        let large = amounts.filter() -> amount {
            amount > 100;
        };
        Stdout().writeln("total " + total.string() + ", large " + large.size().string());
    }
}
```

**Prefer**

```okkama,run
module sales;

import "io/stdout" take Stdout;

// The sales of a day, in cents.
object Sales {
    has amounts is @int;

    // The sales amounts.
    Sales(:amounts);

    // All sales together.
    total() is int {
        amounts.reduce(0) -> sum, amount {
            sum + amount;
        };
    }

    // How many sales were over limit.
    over(limit is int) is int {
        amounts.filter() -> amount {
            amount > limit;
        }.size();
    }
}

main() is int {
    let sales = Sales(amounts: [40, 250, 120]);
    Stdout().writeln("total ", sales.total(), ", large ", sales.over(100));
    0;
}
```
```output
total 410, large 2
```

### 3.2 Constructors bind; actions compute (compiler, review)

**Why.** A constructor that parses, sorts, counts or calls other objects
does work that nobody may need, every time, and hides effects in a place
that cannot be replaced. A constructor binds its fields and checks them
with `confirm`; nothing else. Work moves into an action, and `once let`
keeps its answer for the life of the object. The compiler already refuses
a constructor that calls the object's own actions.

```okkama,fail
module eager;

object Scores {
    has items is @int;

    Scores(items is @int) {
        self.items = items;
        self.best();
    }

    best() is int {
        items.reduce(0) -> top, item {
            if item > top {
                item;
            }
            else {
                top;
            };
        };
    }
}
```
```error
example.ok:8:9: semantic error: Cannot call actions on self before construction completes
```

**Avoid**

```okkama
module eager_ranking;

import "io/stdout" take Stdout;

// Avoid: the constructor computes and writes; both happen even if nobody asks.
object Scores {
    has items is @int;
    has best is int;

    Scores(items is @int) {
        self.items = items;
        self.best = items.reduce(0) -> top, item {
            if item > top {
                item;
            }
            else {
                top;
            };
        };
        Stdout().writeln("scores ready");
    }
}
```

**Prefer**

```okkama
module ranking;

// The scores of a game.
object Scores {
    has items is @int;

    // The scores items.
    Scores(:items);

    // The highest score, computed on the first question and kept.
    best() is int {
        once let top = items.reduce(0) -> top, item {
            if item > top {
                item;
            }
            else {
                top;
            };
        };
        top;
    }
}
```

Three things do stay in a constructor, in std as everywhere: `confirm` checks
of the arguments, the object's own runtime state (a buffer, an `Atomic`),
and a canonical form that `==` depends on (a reduced fraction), because
`once` slots are not part of equality.

### 3.3 An invalid object never exists (review)

**Why.** If a constructor accepts anything and a later `valid()` says
whether the object is usable, every caller must remember to ask, and the
object can travel far before anyone does. Check the arguments in the
constructor with `confirm … else throw`: a refused construction produces no
object, so every object that exists is valid.

**Avoid**

```okkama
module late_check;

// Avoid: any port is accepted; callers must remember to ask valid().
object Port {
    has number is int;

    Port(:number);

    valid() is bool {
        number > 0 && number < 65536;
    }
}
```

**Prefer**

```okkama,run
module ports;

import "io/stdout" take Stdout;

// A TCP port.
object Port {
    has number is int;

    // The port number; refused outside 1..65535.
    Port(number is int) {
        confirm number > 0 && number < 65536 else throw InvalidPort.out_of_range(number);
        self.number = number;
    }

    // The port as it appears after a host: ":8080".
    string() is string {
        ":" + number.string();
    }
}

// A port number that cannot be used.
//
// - `out_of_range`: the number is not in 1..65535.
enum InvalidPort is Error = (out_of_range(number is int));

main() is int {
    Stdout().writeln("localhost", Port(8080));
    Stdout().writeln(try {
        Port(70000).string();
    }
    catch e is InvalidPort {
        e.text();
    });
    0;
}
```
```output
localhost:8080
InvalidPort.out_of_range(70000)
```

### 3.4 Nouns answer, verbs act, adjectives judge (review)

**Why.** A name should say what a call gives back, so a reader knows
whether it is a question or a command without opening it. An action that
answers is named for its answer, a noun (`total()`, `lines()`,
`checksum()`); one that answers yes or no is an adjective (`empty()`,
`expired()`, `supported()`); one called for its effect is a verb and
returns `void` (`write`, `send`, `close`); a successor is named for the
event (`shipped`, `joined`). No `get_`, `is_`, `calculate_`, `do_` or
`process` prefixes: they add nothing.

**Avoid**

```okkama
module cart_names;

// Avoid: prefixes that say nothing, and a verb that answers.
object Cart {
    has items is @int;

    Cart(:items);

    get_total() is int {
        items.reduce(0) -> sum, item {
            sum + item;
        };
    }

    is_empty() is bool {
        items.size() == 0;
    }

    calculate() is int {
        items.size();
    }
}
```

**Prefer**

```okkama
module cart;

// A shopping cart of prices.
object Cart {
    has items is @int;

    // A cart holding items.
    Cart(:items);

    // What the cart costs.
    total() is int {
        items.reduce(0) -> sum, item {
            sum + item;
        };
    }

    // Whether nothing is in the cart.
    empty() is bool {
        items.size() == 0;
    }

    // The cart after one more item.
    added(price is int) is ^Cart {
        ^items = items + [price];
    }
}
```

## 4. Roles and composition

### 4.1 A parameter takes the role it needs (review)

**Why.** A role describes behaviour: what can be asked, not how it is
done. A parameter typed by a concrete object ties the caller to one
implementation and makes tests replace the world around it. Type each
parameter and field by the smallest role that covers what the code calls.
std is full of them: `WritingText` (done by `Stdout` and `TextBuffer`),
`Timing` (the clock), `Sequencing`, `Iterating`.

**Avoid**

```okkama
module concrete_receipt;

import "io/stdout" take Stdout;

// Avoid: the receipt can only ever print to the terminal.
object Receipt {
    has total is int;

    Receipt(:total);

    printed(out is Stdout) is void {
        out.writeln("Total: ", total);
    }
}
```

**Prefer**

```okkama,run
module receipt;

import "io/stdout" take Stdout;
import "io/stream" take WritingText;
import "strings/buffer" take TextBuffer;

// The receipt of a sale.
object Receipt {
    has total is int;

    // A receipt for total cents.
    Receipt(:total);

    // Writes the receipt to out.
    write(out is WritingText) is void {
        out.writeln("Total: ", total);
    }
}

main() is int {
    let receipt = Receipt(total: 410);
    receipt.write(Stdout());
    let page = TextBuffer();
    receipt.write(page);
    Stdout().writeln(page.string().length());
    0;
}
```
```output
Total: 410
11
```

### 4.2 Decorate instead of configuring (review)

**Why.** Okkama has no inheritance, and needs none: behaviour is shared by
holding another object that does the same role. A *decorator* adds one
behaviour around another; decorators combine in any order and a new one
touches no existing code. The alternative, one object with options that
every action reads, grows a branch per option and a test per combination
(10.3).

**Avoid**

```okkama
module price_options;

// Avoid: one object with options that every action must read.
object Price {
    has cents is int;
    has discount is int = 0;
    has taxed is bool = false;

    Price(:cents, :discount, :taxed);

    due() is int {
        let reduced = cents - int(cents * discount / 100);
        if taxed {
            int(reduced * 6 / 5);
        }
        else {
            reduced;
        }
    }
}
```

**Prefer**

```okkama,run
module prices;

import "io/stdout" take Stdout;

// Something that costs money.
role Priced {
    // What is due, in cents.
    due() is int;
}

// A plain price.
object Price does Priced {
    has cents is int;

    // A price of cents.
    Price(:cents);

    // The price itself.
    due() is int {
        cents;
    }
}

// A price with a discount taken off.
object Discounted does Priced {
    has origin is Priced;
    has percent is int;

    // origin with percent off.
    Discounted(:origin, :percent);

    // The price after the discount, rounded down.
    due() is int {
        int(origin.due() * (100 - percent) / 100);
    }
}

// A price with a 20% tax on top.
object Taxed does Priced {
    has origin is Priced;

    // origin with tax added.
    Taxed(:origin);

    // The price with tax, rounded down.
    due() is int {
        int(origin.due() * 6 / 5);
    }
}

main() is int {
    let price is Priced = Taxed(origin: Discounted(origin: Price(cents: 1000), percent: 10));
    Stdout().writeln(price.due());
    0;
}
```
```output
1080
```

A chain of decorators that *every* caller has to assemble is a missing
object (5.1): give the common combination a name of its own.

### 4.3 Dependencies arrive as values (review)

**Why.** An object that builds its own clock, connection or output stream
cannot be tested without the real one, and nothing global can be patched
in Okkama (there are no globals). Take the dependency in the constructor,
typed by a role (4.1). A default for the common case is fine:
`Now(clock is Timing = SystemClock())` in std does exactly that.

**Avoid**

```okkama
module hidden_output;

import "io/stdout" take Stdout;

// Avoid: the report decides for itself where it goes.
object Report {
    has lines is @string;

    Report(:lines);

    publish() is void {
        for lines -> line {
            Stdout().writeln(line);
        }
    }
}
```

**Prefer**

```okkama,test
module report;

import "io/stdout" take Stdout;
import "io/stream" take WritingText;
import "strings/buffer" take TextBuffer;

// Lines that go out together.
object Report {
    has lines is @string;
    has out is WritingText;

    // A report of lines, written to out (the terminal unless told otherwise).
    Report(lines is @string, out is WritingText = Stdout()) {
        self.lines = lines;
        self.out = out;
    }

    // Writes every line.
    publish() is void {
        for lines -> line {
            out.writeln(line);
        }
    }
}

test "a report writes its lines in order" {
    let page = TextBuffer();
    Report(lines: ["a", "b"], out: page).publish();
    test.is(page.string(), "a\nb\n");
}
```
```output
=== RUN   a report writes its lines in order
--- PASS: a report writes its lines in order
PASS
```

## 5. The shape of an API

### 5.1 A short common path, no matryoshkas (review)

**Why.** A common task takes one to three lines from the caller's side.
Three or more constructors nested in one expression to do one thing
(`Number(Utf8Arguments(ProcessArguments()).item(0)).int()`, std before
0.5.4; now `call.int("port")`) means the module is missing an entry point.
Give the task one object, with the parts as named arguments.

**Avoid**

```okkama
module matryoshka;

// Avoid: three wrappers to say one thing; every caller rebuilds the stack.
object Words {
    has text is string;

    Words(:text);

    items() is @string {
        text.split(" ");
    }
}

object Counted {
    has words is Words;

    Counted(:words);

    size() is int {
        words.items().size();
    }
}

object Shown {
    has counted is Counted;

    Shown(:counted);

    text() is string {
        counted.size().string() + " words";
    }
}

main() is int {
    let shown = Shown(counted: Counted(words: Words(text: "to be or not"))).text();
    0;
}
```

**Prefer**

```okkama,run
module word_count;

import "io/stdout" take Stdout;

// A text and how many words it has.
object WordCount {
    has text is string;

    // The count of text's space-separated words.
    WordCount(:text);

    // The number of words.
    count() is int {
        text.split(" ").size();
    }

    // The count as people read it: "4 words".
    string() is string {
        self.count().string() + " words";
    }
}

main() is int {
    Stdout().writeln(WordCount(text: "to be or not"));
    0;
}
```
```output
4 words
```

### 5.2 Parts one by one, never a literal array of parts (review)

**Why.** An array says "any number of the same thing". A name given as
`["Ada", "Lovelace"]` or an address as `["Main St", "5", "Springfield"]`
loses what each part means, accepts the wrong count and the wrong order,
and makes the caller remember the layout. Give each part a named argument.
An array is right for a real list of like things: the lines of an
invoice, the words of a text.

**Avoid**

```okkama
module name_parts;

// Avoid: which part is which is a convention the caller must know.
object Name {
    has parts is @string;

    Name(:parts);

    string() is string {
        parts[1] + ", " + parts[0];
    }
}

main() is int {
    let name = Name(parts: ["Ada", "Lovelace"]);
    0;
}
```

**Prefer**

```okkama,run
module names;

import "io/stdout" take Stdout;

// A person's name.
object Name {
    has given, family is string;

    // The name made of given and family names.
    Name(:given, :family);

    // The name as a catalogue lists it: "Lovelace, Ada".
    string() is string {
        family + ", " + given;
    }
}

main() is int {
    Stdout().writeln(Name(given: "Ada", family: "Lovelace"));
    0;
}
```
```output
Lovelace, Ada
```

### 5.3 Named arguments, with defaults for the common case (compiler, review)

**Why.** At the call site, `Fetch(url: u, retries: 3)` reads; `Fetch(u, 3,
false, 30)` does not. Constructors take named arguments; positional ones are
for actions and for a constructor of one value (`Path("notes.txt")`,
`Port(8080)`). Every part with a sensible default has one, so the plain
construction works and a special case names only what differs. One
constructor with defaults beats a family of overloads. A shorthand
constructor (`Name(:a, :b)`) accepts only named arguments, and no call may
mix the two kinds.

```okkama,fail
module mixed;

object Fetch {
    has url is string;
    has retries is int = 3;

    Fetch(:url, :retries);
}

main() is int {
    let a = Fetch("https://example.org", retries: 1);
    let b = Fetch("https://example.org", 1);
    0;
}
```
```error
example.ok:11:13: semantic error: A call cannot mix positional and named arguments
example.ok:12:13: semantic error: Constructor of "Fetch" takes named arguments only: write Fetch(url: …, retries: …)
```

**Avoid**

```okkama
module positional;

// Avoid: positional flags and numbers; nobody can read the call.
object Fetch {
    has url is string;
    has retries is int;
    has follow is bool;

    Fetch(url is string, retries is int, follow is bool) {
        self.url = url;
        self.retries = retries;
        self.follow = follow;
    }
}

main() is int {
    let fetch = Fetch("https://example.org", 3, false);
    0;
}
```

**Prefer**

```okkama,run
module fetch;

import "io/stdout" take Stdout;

// A request for a URL, retried on failure.
object Fetch {
    has url is string;
    has retries is int = 3;

    // A fetch of url, tried retries more times after a failure (3 unless told).
    Fetch(:url, :retries);

    // The fetch as a log shows it.
    string() is string {
        url + " (" + retries.string() + " retries)";
    }
}

main() is int {
    Stdout().writeln(Fetch(url: "https://example.org"));
    Stdout().writeln(Fetch(url: "https://example.org", retries: 10));
    0;
}
```
```output
https://example.org (3 retries)
https://example.org (10 retries)
```

### 5.4 One name, one meaning, one type (review)

**Why.** Once a reader learns what `lines()` gives, that knowledge must hold
in every object. The same action name never returns an iterator here and an
array there, text in one object and bytes in another. Use the words std
uses: `string()` for text, `content()` for a whole text, `lines()` for its
lines, `size()` for a count of items, `length()` for code points, `close()`
for early release.

**Avoid**

```okkama
module two_meanings;

// Avoid: size() is a count in one object and bytes in the other.
object Shelf {
    has books is @string;

    Shelf(:books);

    size() is int {
        books.size();
    }
}

object Note {
    has text is string;

    Note(:text);

    size() is string {
        text.length().string() + " chars";
    }
}
```

**Prefer**

```okkama
module one_meaning;

// Books on a shelf.
object Shelf {
    has books is @string;

    // A shelf of books.
    Shelf(:books);

    // How many books there are.
    size() is int {
        books.size();
    }
}

// A short note.
object Note {
    has text is string;

    // A note of text.
    Note(:text);

    // How many characters the note has.
    length() is int {
        text.length();
    }
}
```

### 5.5 Walk with `for`, not with `each(fn)` (review)

**Why.** A callback walker (`each(fn)`, `mapped(fn)`) hides the loop:
`next`, `last` and `return` cannot leave it, failures travel through a
closure, and every collection invents its own. Let the object be walked
with `for`: give it `Iterating<T>` (a `cursor()`), or `Batching<T>` for a
stream read in pieces. Then `for`, `map`, `filter`, `reduce` and `find` all
work on it.

**Avoid**

```okkama
module walker;

// Avoid: a closure walker; callers cannot stop early or return from it.
object Playlist {
    has songs is @string;

    Playlist(:songs);

    each(visit is (string) is void) is void {
        for songs -> song {
            visit(song);
        }
    }
}
```

**Prefer**

```okkama,run
module playlist;

import "io/stdout" take Stdout;
import "collections/iteration" take Iterating, Cursor;

// Songs played in order.
object Playlist does Iterating<string> {
    has songs is @string;

    // A playlist of songs.
    Playlist(:songs);

    // The first song's position; `for` walks from it.
    cursor() is Cursor<string> {
        songs.cursor();
    }
}

main() is int {
    for Playlist(songs: ["intro", "theme", "outro"]) -> song {
        last if song == "outro";
        Stdout().writeln(song);
    }
    0;
}
```
```output
intro
theme
```

### 5.6 Never `value()`: the answer has a noun (review)

**Why.** `value()`, `result()`, `get()` and `data()` say nothing about what
comes back. An object that computes one thing names the answer:
`DaysBetween(left: a, right: b).count()`, `Crc32(bytes).checksum()`, and an
adjective for yes or no (`expired()`). The object names
what is asked about and the action names the answer, so neither repeats
the other. `value()` stays only where it is the domain's word: the value of
a name/value pair (a header, a cookie) and the content of a holder (`Cell`,
`Atomic`).

**Avoid**

```okkama
module generic_answer;

// Avoid: value() could be anything; the reader must open the object.
object Distance {
    has metres is int;

    Distance(:metres);

    value() is int {
        int(metres / 1000);
    }
}
```

**Prefer**

```okkama
module distance;

// A distance on the ground.
object Distance {
    has metres is int;

    // A distance of metres.
    Distance(:metres);

    // Whole kilometres, rounded down.
    kilometres() is int {
        int(metres / 1000);
    }
}
```

### 5.7 A closed set is an enum (review)

**Why.** A state, a kind or a command spelled as a string (`"open"`,
`"closed"`) or a magic number (`200`, `404`) compiles for any value,
including the misspelled one. An enum names the cases, carries their data
in payloads, makes a `match` exhaustive, and makes the compiler find every
place a new case must be handled. A number with a registered name is a case
(`Status.not_found`); the open rest goes to a case like `other(code)`.

**Avoid**

```okkama
module stringly;

// Avoid: the status is any string; "opne" compiles and is never true.
object Ticket {
    has status is string;

    Ticket(:status);

    open() is bool {
        status == "open";
    }
}
```

**Prefer**

```okkama
module tickets;

// Where a ticket is in its life.
//
// - `open`: someone has to act on it.
// - `closed`: nothing is left to do.
// - `blocked`: waiting for the ticket numbered `on`.
enum Status = (open, closed, blocked(on is int));

// A ticket in a queue.
object Ticket {
    has status is Status;

    // A ticket in status.
    Ticket(:status);

    // Whether someone can work on the ticket now.
    actionable() is bool {
        match status {
            case Status.open: true;
            case Status.closed: false;
            case Status.blocked(_): false;
        };
    }
}
```

## 6. Errors

### 6.1 One error enum per domain; its cases carry what the caller needs (compiler, review)

**Why.** A caller that wants to react to a failure needs to know which one
it is and the data to act on, not a sentence to parse. In Okkama only an
error enum (`enum E is Error = (…)`) is thrown; there is no `Exception`
class and an object cannot be an error. Give each domain one enum, in a
module below everything that throws it, with cases that name what went
wrong and payloads that carry the path, the value or the limit involved.

```okkama,fail
module error_object;

object Failure does Error {
    has message is string;

    Failure(:message);
}
```
```error
example.ok:3:21: semantic error: Object "Failure" cannot do Error: errors are enums, declare `enum Failure is Error = (...)`
```

**Avoid**

```okkama
module message_error;

// Avoid: one case with a sentence; callers can only print it.
enum TransferError is Error = (failed(message is string));
```

**Prefer**

```okkama
module transfer_error;

// Why a transfer was refused.
//
// - `insufficient`: the account holds `balance`, the transfer needed `amount`.
// - `frozen`: the account numbered `account` takes no transfers.
// - `same_account`: money cannot move from an account to itself.
enum TransferError is Error = (
    insufficient(balance is int, amount is int),
    frozen(account is int),
    same_account
);
```

### 6.2 No sentinel values (review)

**Why.** `-1` for "not found", `""` for "no name" and `0` for "unknown"
look like real answers, so the caller forgets to check, and the wrong value
travels on. When absence is a legitimate answer, the type says so:
`Maybe<T>`. When the caller cannot go on without a value, the action
refuses with a typed error. Decide absence once, at the edge where the
data comes in, and let the objects inside hold values that are there.

**Avoid**

```okkama
module sentinel;

// Avoid: -1 looks like an index; nothing forces the caller to check.
object Roster {
    has names is @string;

    Roster(:names);

    position(name is string) is int {
        names.index_of(name) else -1;
    }
}
```

**Prefer**

```okkama,run
module roster;

import "io/stdout" take Stdout;

// The people on a team, in order.
object Roster {
    has names is @string;

    // A roster of names.
    Roster(:names);

    // Where name stands, if it is on the roster.
    position(name is string) is Maybe<int> {
        names.index_of(name);
    }

    // Where name stands; refused when name is not on the roster.
    place(name is string) is int {
        self.position(name) else throw RosterError.unknown(name);
    }
}

// A name the roster does not have.
//
// - `unknown`: nobody is called `name`.
enum RosterError is Error = (unknown(name is string));

main() is int {
    let team = Roster(names: ["Ada", "Grace"]);
    Stdout().writeln(team.place("Grace"), " ", team.position("Alan") else -1);
    0;
}
```
```output
1 -1
```

The `else -1` in `main` is fine: there the fallback is the caller's
decision, made where it is visible, not a value hidden in a signature.

### 6.3 Catch what you can answer, by name (compiler, review)

**Why.** A catch-all around a loop turns every failure, bugs included,
into a skipped iteration. Okkama has no catch-all and no `finally`
(cleanup is `defer` and `destroy`), and panics and runtime errors cannot be
caught at all. Catch only the refusals you can answer, by type or by case,
and let the rest go up. `catch e is Error` exists for the top of a program
or a request; inside the logic it is the catch-all by another name, and so
is `try? e`, which turns every error into `Maybe.none`: use it only where
any refusal means the same thing.

```okkama,fail
module catch_all;

main() is int {
    try { 1; } catch e { 0; };
    0;
}
```
```error
example.ok:4:24: syntax error at "{": Expect "is" after catch error name.
  |
4 |     try { 1; } catch e { 0; };
  |                        ^
```

**Avoid**

```okkama
module broad_catch;

import "strings/parse" take Number;

// Avoid: every error becomes 0, including the ones that mean a bug.
object Total {
    has texts is @string;

    Total(:texts);

    sum() is int {
        texts.reduce(0) -> sum, text {
            sum + (try {
                Number(text).int();
            }
            catch e is Error {
                0;
            });
        };
    }
}
```

**Prefer**

```okkama,run
module totals;

import "io/stdout" take Stdout;
import "strings/parse" take Number, InvalidInteger;

// Numbers typed by people, some of them not numbers.
object Typed {
    has texts is @string;

    // The typed texts.
    Typed(:texts);

    // The sum of the texts that are integers; the rest count as nothing.
    sum() is int {
        texts.reduce(0) -> sum, text {
            sum + self.amount(text);
        };
    }

    hidden amount(text is string) is int {
        try {
            Number(text).int();
        }
        catch e is InvalidInteger {
            0;
        };
    }
}

main() is int {
    Stdout().writeln(Typed(texts: ["4", "x", "38"]).sum());
    0;
}
```
```output
42
```

### 6.4 Refusals change their names at a boundary (review)

**Why.** If a module lets the errors of the layer below escape, its callers
must know that layer and change when it is replaced. A layer catches the
refusals of the layer below that it understands and refuses in its own
terms, with what its caller needs to act on. A host failure
(`NativeFailure`) is wrapped into the domain's error at the module
boundary.

**Avoid**

```okkama
module leaking;

import "strings/parse" take Number;

// Avoid: callers of Settings must know that strings/parse is underneath.
object Settings {
    has port is string;

    Settings(:port);

    port() is int {
        Number(port).int();
    }
}
```

**Prefer**

```okkama,run
module settings;

import "io/stdout" take Stdout;
import "strings/parse" take Number, InvalidInteger;

// Settings as they were written in a file.
object Settings {
    has port is string;

    // The settings, with port as it was written.
    Settings(:port);

    // The port to listen on.
    listening() is int {
        try {
            Number(port).int();
        }
        catch e is InvalidInteger {
            throw SettingsError.port(port);
        };
    }
}

// A setting that cannot be used.
//
// - `port`: the port setting `text` is not a number.
enum SettingsError is Error = (port(text is string));

main() is int {
    Stdout().writeln(try {
        Settings(port: "eighty").listening().string();
    }
    catch e is SettingsError {
        e.text();
    });
    0;
}
```
```output
SettingsError.port("eighty")
```

### 6.5 A bug is a panic; a refusal is a throw (review)

**Why.** Bad input from outside is expected: the caller can react, so it is
a typed `throw` (`confirm … else throw`). A broken invariant inside the
program is a bug: nobody can handle it, so it stops the run (`confirm c;`
or `confirm c else panic "why";`), and no `catch` sees it. Mixing them up
either crashes on a typo in a config file or lets a caller "handle" a bug.

**Avoid**

```okkama
module panic_on_input;

// Avoid: a user's typo stops the whole program.
object Age {
    has years is int;

    Age(years is int) {
        confirm years >= 0 else panic "negative age";
        self.years = years;
    }
}
```

**Prefer**

```okkama
module age;

// An age that cannot be.
//
// - `negative`: `years` is below zero.
enum InvalidAge is Error = (negative(years is int));

// How old someone is.
object Age {
    has years is int;

    // An age of years; refused below zero.
    Age(years is int) {
        confirm years >= 0 else throw InvalidAge.negative(years);
        self.years = years;
    }

    // The age in whole decades.
    decades() is int {
        int(years / 10);
    }
}
```

### 6.6 A role, not `any` (review)

**Why.** An `any` value can only be stored and passed: nothing can be asked
of it and nothing casts it back. Code that holds `any` plus a string saying
what it "really" is has given up on types. If the code needs to call
something on the value, that something is a role; if the set of shapes is
closed, it is an enum with payloads.

**Avoid**

```okkama
module any_payload;

// Avoid: what the payload is lives in a string next to it.
object Event {
    has kind is string;
    has payload is any;

    Event(:kind, :payload);
}
```

**Prefer**

```okkama
module events;

// Something that happened, as a log line.
role Event {
    // The log line for the event.
    line() is string;
}

// A user signed in.
object SignedIn does Event {
    has user is string;

    // user signed in.
    SignedIn(:user);

    // The log line.
    line() is string {
        "signed in: " + user;
    }
}
```

## 7. Async

Async is for waiting: a task that waits for the network, a timer or another
task lets the others run. One scheduler runs one task at a time, and tasks
switch only at `await`.

### 7.1 `async` only where something waits (review)

**Why.** `async` is part of a signature and spreads to every caller: each
must `await` and be async itself. An action that only computes gains
nothing from it and costs every caller. Make an action async when it waits
for I/O, a timer or another task, and keep the rest synchronous.

**Avoid**

```okkama
module needless_async;

// Avoid: nothing here waits, yet every caller must await.
object Tax {
    has rate is int;

    Tax(:rate);

    async on(amount is int) is int {
        int(amount * rate / 100);
    }
}
```

**Prefer**

```okkama
module tax;

// A tax rate in percent.
object Tax {
    has rate is int;

    // A tax of rate percent.
    Tax(:rate);

    // The tax on amount, rounded down.
    on(amount is int) is int {
        int(amount * rate / 100);
    }
}
```

### 7.2 Every task has an owner (review)

**Why.** Work that outlives the code that started it is the `goto` of
concurrency: no caller knows when it ends or where its failure goes.
Okkama's `await` is structured: a task owns the future it awaits, and
cancelling the task cancels and waits for that future. Keep it that way.
Await what you start; for several children use an owner that waits for all
of them: `AllFutures` (all, in input order), `Raced` and `FirstSuccessful`,
`FutureGroup` for a set that grows, `FutureScope` for a body with its
children (`async/collect`, `async/scope`). Never `spawn` and forget: the
compiler accepts it, and a forgotten task's result is never checked. A
future that is not spawned and never awaited does not even start: calls
of async actions are lazy.

**Avoid**

```okkama
module spawn_and_forget;

import "io/stdout" take Stdout;

// Avoid: the welcome mail runs somewhere; nobody learns whether it failed.
object Mailer {
    Mailer();

    async sent(to is string) is void {
        Stdout().writeln("mail to " + to);
    }
}

object Signup {
    Signup();

    async joined(name is string) is string {
        spawn Mailer().sent(name);
        name;
    }
}
```

**Prefer**

```okkama,run
module signup;

import "io/stdout" take Stdout;
import "async/collect" take AllFutures;
import "collections/sequence" take Sequencing;

// A mail service.
object Mailer {
    // The mailer.
    Mailer();

    // Sends a welcome mail to to; answers the address it went to.
    async sent(to is string) is string {
        Stdout().writeln("mail to " + to);
        to;
    }
}

// New members joining at once.
object Signup {
    has names is @string;

    // A signup of names.
    Signup(:names);

    // Welcomes everyone; done when every mail is out, failed if one failed.
    async welcomed() is int {
        let mails is @Future<string> = names.map() -> name {
            Mailer().sent(name);
        };
        let all is AllFutures<string> = AllFutures(origins: mails);
        let sent is Sequencing<string> = await all.results();
        sent.size();
    }
}

async main() is int {
    let count = await Signup(names: ["ada", "grace"]).welcomed();
    Stdout().writeln(count.string() + " mails");
    0;
}
```
```output
mail to ada
mail to grace
2 mails
```

### 7.3 Never block the scheduler (review)

**Why.** Scheduling is cooperative: a task that computes without awaiting
holds up every other task of the run, and nothing preempts it. In a server,
one request handler that hashes a large file or loops over a big table
stalls every other request. Use the async forms of I/O in async code, and
move heavy synchronous work out: `spawn` of a *synchronous* action runs it
in a worker process, in parallel, and its future is awaited like any other
(only scalars, arrays and maps cross).

**Avoid**

```okkama
module blocking_endpoint;

// Avoid: a long synchronous loop inside a task; every other task waits.
object Endpoint {
    Endpoint();

    async reply(rounds is int) is int {
        (1..rounds).reduce(0) -> sum, n {
            (sum + n * n) % 1000003;
        };
    }
}
```

**Prefer**

```okkama
module worker_endpoint;

// The heavy part of a reply, run in a worker process.
object Digest {
    has rounds is int;

    // A digest of rounds rounds.
    Digest(:rounds);

    // The digest; pure computation, no waiting.
    sum() is int {
        (1..rounds).reduce(0) -> sum, n {
            (sum + n * n) % 1000003;
        };
    }
}

// Answers requests without holding up the others.
object Endpoint {
    // The endpoint.
    Endpoint();

    // The reply for rounds; the computation runs beside this task.
    async reply(rounds is int) is int {
        await spawn Digest(rounds: rounds).sum();
    }
}
```

### 7.4 State between tasks has one owner (compiler, review)

**Why.** An object shared by many tasks and guarded by convention is the
most expensive bug of concurrent code. In Okkama a `Cell` cannot enter
async code and an object cannot hold one, so state that tasks share is
either a small `Atomic` (a counter, a flag) or, for anything larger, the
local state of one task, the *owner*, that the others talk to over a
channel (`async/channel`; Bible 25.9). Between two `await`s a task runs
alone, so a read-then-write with no `await` in between is safe.

```okkama,fail
module shared_cell;

import "cell" take Cell;

object Counter {
    Counter();

    async counted() is int {
        let total = Cell(0);
        total.value();
    }
}
```
```error
example.ok:9:13: semantic error: A Cell cannot be used in an async action: a cell stays in synchronous code; share state between tasks with Atomic
```

**Prefer**

```okkama,run
module visits;

import "io/stdout" take Stdout;
import "async/atomic" take Atomic;

// One visit to a page, counted in a total that many tasks share.
object Visit {
    has total is Atomic<int>;

    // A visit counted in total.
    Visit(:total);

    // Counts the visit; no await between the read and the write.
    async counted() is void {
        total.replace(total.value() + 1);
    }
}

async main() is int {
    let total = Atomic(0);
    let first = spawn Visit(total: total).counted();
    let second = spawn Visit(total: total).counted();
    await first;
    await second;
    Stdout().writeln(total.value());
    0;
}
```
```output
2
```

## 8. Resources

A resource is an object with a `destroy` block: a file, a socket, a lock, a
temporary directory. The scope that owns it destroys it when the scope
ends, however it ends.

### 8.1 Cleanup lives in `destroy`, in the narrowest scope (review)

**Why.** "Call `close()` when done" is a rule every caller must remember on
every path, the early `return` and the `throw` included; the one who
forgets leaks. Put the cleanup in `destroy`: the language runs it once, when
the owning scope ends, after the scope's `defer`s, newest object first.
std resources also offer an idempotent `close()` for releasing early, but
`destroy` stays the guarantee. If the same `defer` appears around every
use of an object, the object wants a `destroy`.

Bind a resource in the narrowest scope that uses it. Bound at the top of a
long action, collected into an array or rebound in a loop (a rebind does
not destroy the old value early), it is held long after its last use. A
loop step is a scope of its own, as the example shows: one row's section
ends before the next one begins.

**Avoid**

```okkama
module close_later;

import "io/stdout" take Stdout;

// Avoid: every caller must remember end(), on every path.
object Section {
    has name is string;

    Section(:name);

    begin() is void {
        Stdout().writeln("begin " + name);
    }

    end() is void {
        Stdout().writeln("end " + name);
    }
}

main() is int {
    let section = Section(name: "import");
    section.begin();
    defer section.end();
    0;
}
```

**Prefer**

```okkama,run
module sections;

import "io/stdout" take Stdout;

// A marked part of a log: its end is written when it goes out of scope.
object Section {
    has name is string;

    // A section called name.
    Section(:name);

    // Writes a line inside the section.
    note(text is string) is void {
        Stdout().writeln("  " + name + ": " + text);
    }

    destroy {
        Stdout().writeln("end " + name);
    }
}

main() is int {
    for ["one", "two"] -> row {
        let section = Section(name: row);
        section.note("imported");
    }
    0;
}
```
```output
  one: imported
end one
  two: imported
end two
```

## 9. Tests

### 9.1 Tests live next to the code (review)

**Why.** A test beside the code it tests is read, run and changed with it.
Write `test "…" { … }` blocks in the module, at module scope or inside the
object they test; `okkama run -t file.ok` runs them, and the code's doc
examples too. Test what an object promises through its actions, with
`test.is` on whole values: a failure then prints both values.

**Avoid**

```okkama
module main_as_test;

import "io/stdout" take Stdout;

// Avoid: a "test" that prints and is checked by eye.
object Fraction {
    has top, bottom is int;

    Fraction(:top, :bottom);

    reduced() is rat {
        top / bottom;
    }
}

main() is int {
    Stdout().writeln(Fraction(top: 2, bottom: 4).reduced().string());
    0;
}
```

**Prefer**

```okkama,test
module fractions;

// A fraction as it was written.
object Fraction {
    has top, bottom is int;

    // top over bottom.
    Fraction(:top, :bottom);

    // The fraction in lowest terms.
    reduced() is rat {
        top / bottom;
    }

    test "a fraction reduces to lowest terms" {
        test.is(Fraction(top: 2, bottom: 4).reduced(), 1 / 2);
    }
}

test "a whole fraction is an integer" {
    test.is(Fraction(top: 6, bottom: 3).reduced(), 2.rat());
}
```
```output
=== RUN   Fraction :: a fraction reduces to lowest terms
--- PASS: Fraction :: a fraction reduces to lowest terms
=== RUN   a whole fraction is an integer
--- PASS: a whole fraction is an integer
PASS
```

### 9.2 Fakes, not mocks of internals (review)

**Why.** A mock that records which calls were made, in which order, tests
how the code works today; the next refactoring breaks it although nothing
the user sees changed. A *fake* is a small working implementation of the
role, declared in the role's own `fake { … }` block and made with
`Role.fake()` in tests only. The test uses it and then asks it what
happened, or checks the outcome. Its changing state lives in an `Atomic`.
(`okkama lint` reports the fake's members as undocumented, #1193; that one
finding is a lint bug, not a style problem.)

**Avoid**

```okkama
module call_log;

import "async/atomic" take Atomic;

// Avoid: the "mock" logs call names; tests then pin the call sequence.
role Store {
    saved(key is string) is void;
}

object MockStore does Store {
    has calls is Atomic<@string>;

    MockStore() {
        calls = Atomic(["connect"]);
    }

    saved(key is string) is void {
        calls.replace(calls.value() + ["begin", "put " + key, "commit"]);
    }

    log() is @string {
        calls.value();
    }
}
```

**Prefer**

```okkama,test
module carts;

import "async/atomic" take Atomic;

// Where carts are kept between visits.
role Carts {
    // Keeps the cart of customer.
    keep(customer is string, items is int) is void;
    // How many items customer's kept cart has; 0 when none is kept.
    items(customer is string) is int;

    fake {
        has kept is Atomic<%string:int> = Atomic([:]);

        keep(customer is string, items is int) is void {
            kept.replace(kept.value() + [customer: items]);
        }

        items(customer is string) is int {
            kept.value()[customer] else 0;
        }
    }
}

// A visit to the shop.
object Visit {
    has carts is Carts;
    has customer is string;

    // A visit of customer, with carts kept in carts.
    Visit(:carts, :customer);

    // Ends the visit, keeping a cart of items.
    left(items is int) is void {
        carts.keep(customer, items);
    }
}

test "a customer's cart is kept when they leave" {
    let carts = Carts.fake();
    Visit(carts: carts, customer: "ada").left(3);
    test.is(carts.items("ada"), 3);
    test.is(carts.items("grace"), 0);
}
```
```output
=== RUN   a customer's cart is kept when they leave
--- PASS: a customer's cart is kept when they leave
PASS
```

### 9.3 A table of cases for a rule with many inputs (review)

**Why.** Ten tests that repeat the same three lines hide the one thing that
differs between them. Put the cases in a table, one row object per case
with named fields, and check every row in one loop. A failed check does
not stop the test, so every wrong row is reported; compare whole rows, so
the report names the row that failed and not only the wrong number. Choose
rows at the edges: the first and last value of each tier, and one on each
side outside the range. (`test.case(name) { … }` gives each row a name of
its own in the report, but `okkama fmt` cannot format a file that uses it
yet, #1192.)

**Avoid**

```okkama
module repeated_tests;

object Postage {
    has grams is int;

    Postage(:grams);

    cents() is int {
        if grams <= 20 {
            85;
        }
        else {
            160;
        }
    }
}

// Avoid: one test per row; the edges hide among the copies.
test "20 g" {
    test.is(Postage(grams: 20).cents(), 85);
}

test "21 g" {
    test.is(Postage(grams: 21).cents(), 160);
}
```

**Prefer**

```okkama,test
module postage;

// What a letter costs to send.
object Postage {
    has grams is int;

    // The postage of a letter of grams.
    Postage(:grams);

    // The price in cents.
    cents() is int {
        if grams <= 20 {
            85;
        }
        else {
            160;
        }
    }
}

// One row of the postage table: a weight and its price.
object Row {
    has grams, cents is int;

    // A letter of grams costs cents.
    Row(:grams, :cents);

    // The row as the tariff prices it; equal to the row when the tariff agrees.
    priced() is Row {
        Row(grams: grams, cents: Postage(grams: grams).cents());
    }
}

test "postage at every edge" {
    let rows is @Row = [
        Row(grams: 1, cents: 85),
        Row(grams: 20, cents: 85),
        Row(grams: 21, cents: 160)
    ];
    for rows -> row {
        test.is(row.priced(), row);
    }
}
```
```output
=== RUN   postage at every edge
--- PASS: postage at every edge
PASS
```

## 10. Anti-patterns

The shapes below compile and still make a program worse. Several have a
rule above; the ones without get one here.

| Anti-pattern | Instead | Rule |
|---|---|---|
| Getters of internals, data bags | an action that answers the caller's question | 1.2, 1.5 |
| Setters, `set_…` successors | a successor named for the event | 2.2 |
| A formatter or decoder around an object | the object answers in the form people use | 1.3 |
| `Manager`, `Helper`, `Util` | small objects named for what they are | 3.1 |
| Matryoshka of constructors | one object with named parts | 5.1 |
| `each(fn)` walkers | `Iterating` or `Batching`, then `for` | 5.5 |
| `value()`, `result()`, `get()` | the noun of the answer | 5.6 |
| Stringly typed states and commands | an enum | 5.7 |
| Sentinel values | `Maybe` or a typed refusal | 6.2 |
| Catching everything to continue | catch the refusal you can answer | 6.3 |
| `any` with a kind string | a role or an enum | 6.6 |
| Spawn and forget | await, or an owner (`AllFutures`, `FutureScope`) | 7.2 |
| Close-later protocols | `destroy` | 8.1 |
| Mocks that pin call sequences | a fake in the role | 9.2 |
| Utility objects | an object for the concept | 10.1 |
| Flag arguments | two actions, or an enum | 10.2 |
| Boolean mode fields | decorators or an enum | 10.3 |
| A `match` on a kind where a role fits | a role with an object per kind | 10.4 |
| A god object | one object per responsibility, wired in `main` | 3.1 |
| A `Cell` threaded through actions | a successor chain | 2.3 |

### 10.1 Utility objects (review)

**Why.** An object with no fields whose actions take all their data as
parameters (`StringUtils().capitalized(text)`, `MathHelper().percent(a,
b)`) is a bag of free functions under an object's name. The knowledge has
no home, and the next helper repeats it. Make the data the object, and the
helper its action: `Title(text).capitalized()`. Look in std first
(`okkama man -k`); it probably has it.

**Avoid**

```okkama
module text_utils;

// Avoid: a namespace of procedures pretending to be an object.
object TextUtils {
    TextUtils();

    initials(given is string, family is string) is string {
        given.slice(0, 1) + family.slice(0, 1);
    }

    shouted(text is string) is string {
        text.upper() + "!";
    }
}
```

**Prefer**

```okkama
module person;

// A person, by name.
object Person {
    has given, family is string;

    // A person called given family.
    Person(:given, :family);

    // The initials: "AL" for Ada Lovelace.
    initials() is string {
        given.slice(0, 1) + family.slice(0, 1);
    }
}
```

### 10.2 Flag arguments (review)

**Why.** `export(report, true)` cannot be read without the declaration, and
an action with a boolean that chooses what it does is two actions sharing
a name. Split it into two actions named for what each does, or, when the
choice is data that travels, take an enum.

**Avoid**

```okkama
module flag_argument;

// Avoid: what does true mean at the call site?
object Amount {
    has cents is int;

    Amount(:cents);

    text(short is bool) is string {
        if short {
            int(cents / 100).string();
        }
        else {
            int(cents / 100).string() + " EUR";
        }
    }
}
```

**Prefer**

```okkama
module amount;

// An amount of euros, in cents.
object Amount {
    has cents is int;

    // An amount of cents.
    Amount(:cents);

    // The amount with its currency: "12 EUR".
    string() is string {
        self.figure() + " EUR";
    }

    // The bare number of whole euros: "12".
    figure() is string {
        int(cents / 100).string();
    }
}
```

### 10.3 Boolean mode fields (review)

**Why.** An object with `verbose`, `json`, `strict` fields is several
objects in one, and its actions branch on every combination, including the
ones that make no sense (`json: true, colour: true`). A closed set of modes
is an enum; behaviours that combine are decorators of a role (4.2).

**Avoid**

```okkama
module mode_flags;

// Avoid: two booleans give four modes; one of them is nonsense.
object Log {
    has json, plain is bool;

    Log(:json, :plain);

    line(text is string) is string {
        if json {
            "{\"msg\":\"" + text + "\"}";
        }
        else {
            text;
        }
    }
}
```

**Prefer**

```okkama
module log_format;

// How a log line is written.
//
// - `plain`: the text as it is.
// - `json`: one JSON object per line.
enum Format = (plain, json);

// A log that writes lines in one format.
object Log {
    has format is Format;

    // A log writing in format.
    Log(:format);

    // The line for text.
    line(text is string) is string {
        match format {
            case Format.plain: text;
            case Format.json: "{\"msg\":\"" + text + "\"}";
        };
    }
}
```

### 10.4 A `match` on a kind where a role fits (review)

**Why.** A "kind" field and a `match` over it in every action means that a
new kind touches every action, and the data of all kinds lives in one
object with fields that are meaningless for most of them. When each kind
has its own data and behaviour, make a role and one object per kind. An
enum with a `match` is right the other way round: a closed set of cases,
many operations over them, the cases rarely change (a parsed command, a
protocol message, the effects a core decided).

**Avoid**

```okkama
module kind_match;

// Avoid: every kind's fields in one object, a match in every action.
enum Kind = (circle, square);

object Shape {
    has kind is Kind;
    has radius, side is int;

    Shape(:kind, :radius, :side);

    area() is rat {
        match kind {
            case Kind.circle: (3 * radius * radius).rat();
            case Kind.square: (side * side).rat();
        };
    }
}
```

**Prefer**

```okkama
module shapes;

// A plane figure.
role Shape {
    // The area of the figure.
    area() is rat;
}

// A circle, with pi taken as 3.
object Circle does Shape {
    has radius is int;

    // A circle of radius.
    Circle(:radius);

    // The area.
    area() is rat {
        (3 * radius * radius).rat();
    }
}

// A square.
object Square does Shape {
    has side is int;

    // A square of side.
    Square(:side);

    // The area.
    area() is rat {
        (side * side).rat();
    }
}
```

## 11. Pragmatic exceptions

These are the places where the rules above bend. They are listed so that
bending them is a decision, not a habit; each one stays at its place and
does not spread into the rest of the program.

### 11.1 Records at the boundaries

Data that crosses the program's edge has the shape of the format, not of
the domain: a JSON document, a database row, a CSV line, a wire message, a
form. At the edge, reading fields by name is the job (`order.int("id")`,
`row.text("name")`), and an object that mirrors the record is acceptable.
Convert to domain objects right there, in one place, and decide absence
there too (6.2); the code past the edge sees only domain objects.

```okkama
module order_import;

import "json" take ParsedJson;

// A line of an order, in the domain's terms.
object Line {
    has sku is string;
    has quantity is int;

    // quantity items of sku.
    Line(:sku, :quantity);

    // Whether the line asks for more than limit items.
    bulk(limit is int) is bool {
        quantity > limit;
    }
}

// An order as it arrives: JSON text from the shop's front end.
object Incoming {
    has text is string;

    // The incoming order text.
    Incoming(:text);

    // The order's lines; the JSON field names stop here.
    lines() is @Line {
        ParsedJson(text).root().list("items").map() -> item {
            Line(sku: item.text("sku"), quantity: item.int("qty"));
        };
    }
}
```

### 11.2 Hot loops

A measured hot path may use what the rest of the code avoids: a `Cell`
accumulator, an index loop over a range, plain arrays and maps instead of
small objects, one wide object instead of a decorator chain. Keep it
inside one action, behind an ordinary interface, with a comment that says
it was measured. Measure first: successors and small objects are cheaper
than they look, and the compiler optimizes the ordinary forms.

```okkama
module histogram;

import "cell" take Cell;

// How often each byte occurs in a block.
object Histogram {
    has bytes is @int;

    // A histogram of bytes, each 0..255.
    Histogram(:bytes);

    // The count of each byte value, index = value.
    counts() is @int {
        // Measured hot path: one array rebuilt per byte is cheaper here than a map.
        let table = Cell((0..256).map() -> n {
            0;
        });
        for bytes -> b {
            table.replace(table.value().replaced(b, table.value()[b] + 1));
        }
        table.value();
    }
}
```

### 11.3 Interop

Natives, helper processes and foreign formats come with their own shapes:
handles, status codes, flat argument lists, strings for everything. Follow
the foreign shape at the seam and nowhere else. The module that wraps it
turns codes into enums, failures (`NativeFailure`) into the domain's
error, and handles into resources with a `destroy`, so its callers never
see the foreign side.

### 11.4 The domain's own words

`value()` is right where it is the domain's word: the value of a
name/value pair (a header, a cookie, a query parameter) and the content of
a holder (`Cell`, `Atomic`). An accessor is acceptable when the value
really is what callers need and no question of theirs can replace it,
such as an identifier used as a map key; ask first whether a cap (1.5) or
an action would do.

### 11.5 Small programs, tests and `main`

A fifty-line tool with two options and one concrete dependency is not
wrong: decorators, roles and fakes pay back as a program grows and as
other people change it. `main` is wiring: it builds the objects and connects
them, and may be plain and procedural. Test code may build rows and
fixtures as simple records (9.3).

## 12. What the tools check

Run them after every change (`okkama agents workflow`):

- `okkama fmt -w FILE.ok`: the one layout (4-space indentation, one member
  or statement per line, a blank line between members). Never format by
  hand.
- `okkama lint FILE.ok` reports:
  - `missing-docs`: a public declaration without a doc comment (objects,
    roles, enums with their cases, constructors, actions, caps). Write one
    line saying what it is or does, as in the examples above.
  - `snake-case`: a local, parameter, field or action not in `snake_case`.
  - `unused-import`, `unused-let`, `unused-hidden`, `unreachable`.
  - `hidden-error`: a public action throws a `hidden` error enum, which
    callers cannot catch by type.
  - `non-exhaustive-match`, `doc-note`, `unresolved-doc-link`.
- `okkama check` enforces what this guide marks **compiler**: private and
  immutable fields, no `with…` actions, typed errors, no catch-all, named
  arguments for shorthand constructors, no mixed calls, no `Cell` in fields
  or async code, no self calls in constructors.

## 13. Checklist

Before calling the work done:

- [ ] `okkama check`, `okkama fmt --check`, `okkama lint` and `okkama run -t`
      are clean.
- [ ] No object hands out its fields; callers ask for results (1.2).
- [ ] Objects answer in the form callers use; no formatter fences (1.3).
- [ ] Helpers are `hidden`; implementations hide behind roles (1.4).
- [ ] Changes are successors named for events; no setters (2.1, 2.2).
- [ ] Each object has one job and a name that says it; constructors only
      bind and `confirm` (3.1–3.3).
- [ ] Answers are nouns, yes/no are adjectives, effects are verbs (3.4).
- [ ] Parameters and fields are typed by roles; dependencies come in (4.1, 4.3).
- [ ] Constructors take named arguments; no arrays of parts; no
      matryoshkas (5.1–5.3).
- [ ] No `each(fn)`, no `value()`, no strings for closed sets (5.5–5.7).
- [ ] Failures are typed, specific, translated at boundaries; no sentinel
      values (6).
- [ ] Every started task is awaited or owned; nothing heavy runs inside a
      task (7.2, 7.3).
- [ ] Cleanup is in `destroy`; resources live in the narrowest scope (8.1).
- [ ] Every public object has a `test` next to it; dependencies are faked
      through their roles (9).
- [ ] Any exception to these rules is one of section 11, and says so in a
      comment.
