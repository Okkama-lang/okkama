// Go counterparts of the bench/micro ownership pairs (same work, same checksums), protocol micro-v2.
// A destroy is `defer Close()`, Go's idiom for scoped cleanup.
package main

import (
	"fmt"
	"os"
	"strconv"
	"time"
)

type Point struct{ x, y int }

func (p *Point) Sum() int            { return p.x + p.y }
func (p *Point) Moved(dx int) *Point { return &Point{p.x + dx, p.y} }

type Res struct{ id int }

func (r *Res) Close() {}

type Maker struct{}

func (m *Maker) Make(i int) int {
	r := &Res{i}
	defer r.Close()
	return r.id
}

type Resource struct{ count *int }

func (r *Resource) Close() { *r.count++ }

type Sink struct{}

func (s *Sink) Touch() {}

type Deferring struct{}

func (d *Deferring) Value(i int) int {
	defer (&Sink{}).Touch()
	return i % 7
}

func baseline(n int) int {
	acc := 0
	for i := 0; i < n; i++ {
		acc += i % 7
	}
	return acc
}

func constructor(n int) int {
	acc := 0
	for i := 0; i < n; i++ {
		acc += (&Point{i, 1}).Sum() % 7
	}
	return acc
}

func successor(n int) int {
	acc := 0
	for i := 0; i < n; i++ {
		acc += (&Point{i, 1}).Moved(2).Sum() % 7
	}
	return acc
}

func destroy(n int) int {
	acc := 0
	for i := 0; i < n; i++ {
		acc += (&Maker{}).Make(i) % 7
	}
	return acc
}

func observed(count *int, i int) int {
	r := &Resource{count}
	defer r.Close()
	return i % 7
}

func destroyObserved(n int) int {
	count, acc := 0, 0
	for i := 0; i < n; i++ {
		acc += observed(&count, i)
	}
	return acc + count
}

func deferCase(n int) int {
	acc := 0
	for i := 0; i < n; i++ {
		acc += (&Deferring{}).Value(i)
	}
	return acc
}

func main() {
	cases := map[string]func(int) int{"baseline": baseline, "constructor": constructor, "successor": successor,
		"destroy": destroy, "destroy-observed": destroyObserved, "defer": deferCase}
	f := cases[os.Args[1]]
	n, _ := strconv.Atoi(os.Args[2])
	warm, _ := strconv.Atoi(os.Args[3])
	w := f(warm)
	start := time.Now()
	c := f(n)
	fmt.Println("micro-v2", n, warm, time.Since(start).Nanoseconds(), c, w)
}
