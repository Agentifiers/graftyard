package main

import (
	"math"
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
