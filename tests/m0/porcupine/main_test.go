package main

import (
	"fmt"
	"math"
	"math/rand/v2"
	"sort"
	"testing"

	"github.com/anishathalye/porcupine"
)

func s(v string) *string { return &v }
func put(c int, call, ret int64, k, v string) porcupine.Operation {
	return porcupine.Operation{ClientId: c, Input: Op{Kind: "put", Key: k, Value: v}, Call: call, Output: output{ok: true}, Return: ret}
}
func get(c int, call, ret int64, k string, v *string) porcupine.Operation {
	return porcupine.Operation{ClientId: c, Input: Op{Kind: "get", Key: k}, Call: call, Output: output{value: v}, Return: ret}
}

func check(t *testing.T, ops []porcupine.Operation) porcupine.CheckResult {
	t.Helper()
	return porcupine.CheckOperationsTimeout(model, ops, 0)
}

// A read that starts after a write returns must see it, or a later one.
func TestStaleReadAfterAckedWriteIsIllegal(t *testing.T) {
	ops := []porcupine.Operation{put(0, 0, 10, "k", "a"), put(0, 20, 30, "k", "b"), get(1, 40, 50, "k", s("a"))}
	if r := check(t, ops); r != porcupine.Illegal {
		t.Fatalf("got %s", r)
	}
}

// The same read overlapping the second write may see either value.
func TestOverlappingReadMaySeeEither(t *testing.T) {
	ops := []porcupine.Operation{put(0, 0, 10, "k", "a"), put(0, 20, 50, "k", "b"), get(1, 30, 40, "k", s("a"))}
	if r := check(t, ops); r != porcupine.Ok {
		t.Fatalf("got %s", r)
	}
}

// A write that got no reply may have happened: a later read of its value is legal, and so is a read of the old one.
func TestNoReplyWriteMayOrMayNotTakeEffect(t *testing.T) {
	lost := porcupine.Operation{ClientId: 0, Input: Op{Kind: "put", Key: "k", Value: "b"}, Call: 20, Output: output{unknown: true}, Return: math.MaxInt64}
	for _, seen := range []string{"a", "b"} {
		ops := []porcupine.Operation{put(0, 0, 10, "k", "a"), lost, get(1, 40, 50, "k", s(seen))}
		if r := check(t, ops); r != porcupine.Ok {
			t.Fatalf("read %s: got %s", seen, r)
		}
	}
}

// A no-reply write cannot flip back: once a read sees it, a later read of the old value is a violation.
func TestNoReplyWriteCannotUnhappen(t *testing.T) {
	lost := porcupine.Operation{ClientId: 0, Input: Op{Kind: "put", Key: "k", Value: "b"}, Call: 20, Output: output{unknown: true}, Return: math.MaxInt64}
	ops := []porcupine.Operation{put(0, 0, 10, "k", "a"), lost, get(1, 40, 50, "k", s("b")), get(1, 60, 70, "k", s("a"))}
	if r := check(t, ops); r != porcupine.Illegal {
		t.Fatalf("got %s", r)
	}
}

// Two compare-and-sets from the same old value cannot both succeed.
func TestDoubleCasSuccessIsIllegal(t *testing.T) {
	cas := func(c int, call, ret int64, v string) porcupine.Operation {
		return porcupine.Operation{ClientId: c, Input: Op{Kind: "cas", Key: "k", Expect: s("a"), Value: v}, Call: call, Output: output{ok: true}, Return: ret}
	}
	ops := []porcupine.Operation{put(0, 0, 10, "k", "a"), cas(1, 20, 40, "x"), cas(2, 25, 45, "y")}
	if r := check(t, ops); r != porcupine.Illegal {
		t.Fatalf("got %s", r)
	}
}

// Keys are independent: a violation on one key is found among many clean ones.
func TestViolationFoundAmongManyKeys(t *testing.T) {
	var ops []porcupine.Operation
	for i := 0; i < 200; i++ {
		k := string(rune('A' + i%26)) + string(rune('a'+i/26))
		ops = append(ops, put(i%7, int64(i*10), int64(i*10+5), k, "v"), get(i%5, int64(i*10+6), int64(i*10+8), k, s("v")))
	}
	ops = append(ops, put(0, 5000, 5010, "Aa", "w"), get(1, 5020, 5030, "Aa", s("v")))
	if r := check(t, ops); r != porcupine.Illegal {
		t.Fatalf("got %s", r)
	}
}

func noReply(c int, call int64, op Op) porcupine.Operation {
	return porcupine.Operation{ClientId: c, Input: op, Call: call, Output: output{unknown: true}, Return: math.MaxInt64}
}

// A no-reply write that a later read saw took effect: segmented mode may not drop it (the 2000-seed sim sweep's
// HARSH history read Illegal under -seg and Ok whole for this reason).
func TestSegmentedKeepsNoReplyWriteThatWasSeen(t *testing.T) {
	ops := []porcupine.Operation{put(0, 0, 10, "k", "a"), noReply(1, 20, Op{Kind: "put", Key: "k", Value: "b"}), get(2, 40, 50, "k", s("b"))}
	if r, _ := segmented(ops, 1, 0); r != porcupine.Ok {
		t.Fatalf("got %s", r)
	}
}

// One nobody saw may still have taken effect, inside the segment that needs it: here only it explains the failed cas.
func TestSegmentedLendsUnseenNoReplyWrite(t *testing.T) {
	ops := []porcupine.Operation{put(0, 0, 10, "k", "a"), noReply(1, 20, Op{Kind: "put", Key: "k", Value: "c"}), casFails(2, 40, 50)}
	if r, _ := segmented(ops, 1, 0); r != porcupine.Ok {
		t.Fatalf("got %s", r)
	}
}

// It takes effect at most once, so two segments that each need it cannot both be passed with it.
func TestSegmentedLendsNoReplyWriteOnce(t *testing.T) {
	ops := []porcupine.Operation{put(0, 0, 10, "k", "a"), noReply(1, 20, Op{Kind: "put", Key: "k", Value: "c"}), casFails(2, 40, 50),
		put(0, 60, 70, "k", "a"), casFails(2, 80, 90)}
	if porcupine.CheckOperations(model, ops) {
		t.Fatal("whole history: want Illegal")
	}
	if r, _ := segmented(ops, 1, 0); r == porcupine.Ok {
		t.Fatalf("got %s", r)
	}
}

func casFails(c int, call, ret int64) porcupine.Operation {
	return porcupine.Operation{ClientId: c, Input: Op{Kind: "cas", Key: "k", Expect: s("a"), Value: "b"}, Call: call, Output: output{ok: false}, Return: ret}
}

// A cut on one key says nothing about another key's state.
func TestSegmentedCutsEachKeyOnItsOwn(t *testing.T) {
	legal := []porcupine.Operation{put(0, 0, 10, "A", "x"), put(1, 20, 30, "B", "y"), get(2, 40, 50, "A", s("x"))}
	if r, _ := segmented(legal, 1, 0); r != porcupine.Ok {
		t.Fatalf("legal: got %s", r)
	}
	stale := []porcupine.Operation{put(0, 0, 10, "B", "b"), put(1, 20, 30, "A", "y"), get(2, 40, 50, "B", s("y"))}
	if r, _ := segmented(stale, 1, 0); r != porcupine.Illegal {
		t.Fatalf("B read A's value: got %s", r)
	}
}

// Segmented mode answers what the whole-history check answers, on random histories over a few keys with puts, gets,
// compare-and-sets, no-reply writes, and quiet gaps where cuts can fall. Half have one output changed.
func TestSegmentedAgreesWithWholeHistory(t *testing.T) {
	rng := rand.New(rand.NewPCG(1, 2))
	count := map[string]int{}
	for n := 0; n < 3000; n++ {
		ops := randomHistory(rng)
		whole := porcupine.CheckOperations(model, ops)
		for _, target := range []int{1, 4} {
			seg, _ := segmented(ops, target, 0)
			count[fmt.Sprintf("whole %v seg %s", whole, seg)]++
			if seg == porcupine.Unknown {
				continue
			}
			if (seg == porcupine.Ok) != whole {
				t.Fatalf("history %d, -seg %d: whole %v, segmented %s\n%v", n, target, whole, seg, ops)
			}
		}
	}
	t.Log(count)
	if count["whole true seg Ok"] < 1000 || count["whole false seg Illegal"] < 1000 || count["whole true seg Unknown"]+count["whole false seg Unknown"] > 60 {
		t.Fatalf("too few decided histories: %v", count)
	}
}

func randomHistory(rng *rand.Rand) []porcupine.Operation {
	type ev struct {
		at int64
		i  int
	}
	keys := []string{"A", "B", "C"}[:1+rng.IntN(3)]
	var ops []porcupine.Operation
	var evs []ev
	var at int64
	written := map[string][]string{}
	for i := 0; i < 8+rng.IntN(20); i++ {
		at += int64(rng.IntN(6))
		if rng.IntN(6) == 0 {
			at += 30 // a quiet gap
		}
		k := keys[rng.IntN(len(keys))]
		op := Op{Key: k}
		switch rng.IntN(3) {
		case 0:
			op.Kind = "get"
		case 1:
			op.Kind, op.Value = "put", fmt.Sprint(i)
		default:
			op.Kind, op.Value = "cas", fmt.Sprint(i)
			if w := written[k]; len(w) > 0 && rng.IntN(4) > 0 {
				op.Expect = s(w[rng.IntN(len(w))])
			}
		}
		if op.Kind != "get" {
			written[k] = append(written[k], op.Value)
		}
		ret := at + 1 + int64(rng.IntN(12))
		ops = append(ops, porcupine.Operation{ClientId: i, Input: op, Call: at, Output: output{}, Return: ret})
		if op.Kind != "get" && rng.IntN(6) == 0 {
			ops[i].Output, ops[i].Return = output{unknown: true}, math.MaxInt64
			if rng.IntN(2) == 0 {
				continue // never took effect
			}
			ret = at + 1 + int64(rng.IntN(60))
		}
		evs = append(evs, ev{at + rng.Int64N(ret-at+1), i})
	}
	sort.Slice(evs, func(a, b int) bool { return evs[a].at < evs[b].at })
	state := map[string]*string{}
	for _, e := range evs {
		o := &ops[e.i]
		op, cur := o.Input.(Op), state[o.Input.(Op).Key]
		matches := op.Kind == "put" || (op.Kind == "cas" && eq(op.Expect, cur))
		if o.Output.(output).unknown {
			// keeps its unknown output
		} else if op.Kind == "get" {
			o.Output = output{value: cur}
		} else {
			o.Output = output{ok: matches}
		}
		if matches {
			v := op.Value
			state[op.Key] = &v
		}
	}
	if rng.IntN(2) == 0 {
		o := &ops[rng.IntN(len(ops))]
		if out := o.Output.(output); !out.unknown {
			if o.Input.(Op).Kind == "get" {
				w := written[o.Input.(Op).Key]
				if out.value == nil && len(w) > 0 {
					out.value = s(w[rng.IntN(len(w))])
				} else {
					out.value = nil
				}
			} else {
				out.ok = !out.ok
			}
			o.Output = out
		}
	}
	return ops
}
