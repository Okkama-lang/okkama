// Ownership (#908): the work of ownership.ok in Go. 100000 records in a slice and on 1000 shelves of a map,
// 300 rounds, each building a value that refers to them. Go has no destroy: the Vault is a plain value.
package main

import (
	"fmt"
	"os"
	"strconv"
	"time"
)

type Ledger interface{ Balance() int }

type Account struct{ amount int }

func (a *Account) Balance() int { return a.amount }

type Vault struct{ amount int }

func (v *Vault) Balance() int { return v.amount * 2 }

type Record struct {
	id     int
	ledger Ledger
}

func (r *Record) Total() int { return r.id%97 + r.ledger.Balance() }

type View struct {
	records []*Record
	shelves map[int][]*Record
	at      int
}

func (v *View) Total() int {
	shelf, ok := v.shelves[v.at%len(v.shelves)]
	if !ok {
		shelf = v.records
	}
	return v.records[v.at].Total() + shelf[v.at%len(shelf)].Total()
}

type Store struct {
	records []*Record
	shelves map[int][]*Record
}

func NewStore(racks int) *Store {
	records := make([]*Record, 0, racks*100)
	for i := 0; i < racks*100; i++ {
		var ledger Ledger
		if i%1000 == 0 {
			ledger = &Vault{i % 13}
		} else {
			ledger = &Account{i % 13}
		}
		records = append(records, &Record{i, ledger})
	}
	shelves := make(map[int][]*Record, racks)
	for shelf := 0; shelf < racks; shelf++ {
		shelves[shelf] = records[shelf*100 : shelf*100+100]
	}
	return &Store{records, shelves}
}

func (s *Store) Sum(rounds int) int {
	total := 0
	for round := 0; round < rounds; round++ {
		fee := &Vault{round % 5}
		view := &View{s.records, s.shelves, round * 7919 % len(s.records)}
		total += view.Total() + fee.Balance()
	}
	return total
}

func main() {
	repeats, _ := strconv.Atoi(os.Args[1])
	result := 0
	for i := 0; i < repeats; i++ {
		start := time.Now()
		result = NewStore(1000).Sum(300)
		fmt.Fprintln(os.Stderr, "iteration", time.Since(start).Nanoseconds())
	}
	fmt.Println(result)
}
