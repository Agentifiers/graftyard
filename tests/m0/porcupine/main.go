// Command m0check checks an M0 history for linearizability with Porcupine (build/BUILD-PLAN.md, M0-WP1 tools; B0-11).
//
// Input is JSON lines, one operation each, as graft-m0-tools records them (src/tools.ts, HistoryRow):
//
//	{"client":3,"call":1730000000000,"ret":1730000000042,"op":{"kind":"put","key":"k1","value":"3.7"},"ok":true,"out":{"ok":true}}
//
// The model is one register per key, partitioned by key, with put, get, and compare-and-set. A row with ok=false got no
// reply, so its effect is unknown: Porcupine treats it as pending until the end of the history, which lets the write
// take effect or not. A failed read carries no information and is dropped.
//
// Usage: go run . [-timeout 10m] [-viz out.html] [-seg N] history.jsonl...
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"os"
	"sort"
	"time"

	"github.com/anishathalye/porcupine"
)

type Op struct {
	Kind   string  `json:"kind"`
	Key    string  `json:"key"`
	Value  string  `json:"value"`
	Expect *string `json:"expect"`
}

type Out struct {
	Value *string `json:"value"`
	Ok    *bool   `json:"ok"`
}

type Row struct {
	Client int   `json:"client"`
	Call   int64 `json:"call"`
	Ret    int64 `json:"ret"`
	Op     Op    `json:"op"`
	Ok     bool  `json:"ok"`
	Out    *Out  `json:"out"`
}

// output is what the register returned. unknown marks an operation with no reply.
type output struct {
	unknown bool
	value   *string
	ok      bool
}

// The state is the key's current value; nil means never written.
func step(state, input, out interface{}) (bool, interface{}) {
	cur := state.(*string)
	op := input.(Op)
	o := out.(output)
	switch op.Kind {
	case "get":
		return eq(o.value, cur), cur
	case "put":
		v := op.Value
		return true, &v
	case "cas":
		matches := eq(op.Expect, cur)
		if o.unknown {
			if matches {
				v := op.Value
				return true, &v
			}
			return true, cur
		}
		if o.ok {
			if !matches {
				return false, cur
			}
			v := op.Value
			return true, &v
		}
		return !matches, cur
	}
	return false, cur
}

func eq(a, b *string) bool {
	if a == nil || b == nil {
		return a == nil && b == nil
	}
	return *a == *b
}

func describe(s *string) string {
	if s == nil {
		return "∅"
	}
	return *s
}

var model = porcupine.Model{
	Partition: func(h []porcupine.Operation) [][]porcupine.Operation {
		byKey := map[string][]porcupine.Operation{}
		var order []string
		for _, o := range h {
			k := o.Input.(Op).Key
			if _, seen := byKey[k]; !seen {
				order = append(order, k)
			}
			byKey[k] = append(byKey[k], o)
		}
		out := make([][]porcupine.Operation, 0, len(order))
		for _, k := range order {
			out = append(out, byKey[k])
		}
		return out
	},
	Init:  func() interface{} { return (*string)(nil) },
	Step:  step,
	Equal: func(a, b interface{}) bool { return eq(a.(*string), b.(*string)) },
	DescribeOperation: func(input, out interface{}) string {
		op, o := input.(Op), out.(output)
		res := "?"
		if !o.unknown {
			if op.Kind == "get" {
				res = describe(o.value)
			} else {
				res = fmt.Sprint(o.ok)
			}
		}
		switch op.Kind {
		case "get":
			return fmt.Sprintf("get(%s) -> %s", op.Key, res)
		case "put":
			return fmt.Sprintf("put(%s, %s) -> %s", op.Key, op.Value, res)
		}
		return fmt.Sprintf("cas(%s, %s => %s) -> %s", op.Key, describe(op.Expect), op.Value, res)
	},
	DescribeState: func(s interface{}) string { return describe(s.(*string)) },
}

func load(paths []string) ([]porcupine.Operation, int, error) {
	var ops []porcupine.Operation
	dropped := 0
	for _, p := range paths {
		f, err := os.Open(p)
		if err != nil {
			return nil, 0, err
		}
		sc := bufio.NewScanner(f)
		sc.Buffer(make([]byte, 1<<20), 1<<20)
		for line := 1; sc.Scan(); line++ {
			if len(sc.Bytes()) == 0 {
				continue
			}
			var r Row
			if err := json.Unmarshal(sc.Bytes(), &r); err != nil {
				f.Close()
				return nil, 0, fmt.Errorf("%s:%d: %w", p, line, err)
			}
			o := output{}
			ret := r.Ret
			switch {
			case !r.Ok && r.Op.Kind == "get":
				dropped++
				continue
			case !r.Ok:
				o.unknown = true
				ret = math.MaxInt64 // may have taken effect at any time after the call
			case r.Op.Kind == "get":
				if r.Out != nil {
					o.value = r.Out.Value
				}
			default:
				o.ok = r.Out != nil && r.Out.Ok != nil && *r.Out.Ok
			}
			ops = append(ops, porcupine.Operation{ClientId: r.Client, Input: r.Op, Call: r.Call, Output: o, Return: ret})
		}
		f.Close()
		if err := sc.Err(); err != nil {
			return nil, 0, err
		}
	}
	return ops, dropped, nil
}

func main() {
	timeout := flag.Duration("timeout", 10*time.Minute, "give up and report Unknown after this long")
	viz := flag.String("viz", "", "write Porcupine's HTML visualization here")
	seg := flag.Int("seg", 0, "check each key in segments of at least this many ops, cut at isolated ops (see segmented); 0 checks it whole")
	flag.Parse()
	ops, dropped, err := load(flag.Args())
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	unknown := 0
	for _, o := range ops {
		if o.Output.(output).unknown {
			unknown++
		}
	}
	summary := map[string]interface{}{"operations": len(ops), "noReplyWrites": unknown, "droppedFailedReads": dropped}
	var res porcupine.CheckResult
	if *seg > 0 {
		res, summary["segments"] = segmented(ops, *seg, *timeout)
	} else {
		var info porcupine.LinearizationInfo
		res, info = porcupine.CheckOperationsVerbose(model, ops, *timeout)
		if *viz != "" {
			if err := porcupine.VisualizePath(model, info, *viz); err != nil {
				fmt.Fprintln(os.Stderr, err)
			}
		}
	}
	summary["result"] = res
	out, _ := json.Marshal(summary)
	fmt.Println(string(out))
	if res != porcupine.Ok {
		os.Exit(1)
	}
}

// Segmented check, per key. Each key's history is cut after an isolated op (every earlier op returned before it was
// called, every later op was called after it returned) whose result names the state: every linearization puts it
// between the two sides, so the next segment starts from the state it leaves.
//
// A no-reply write would overlap everything after its call, so it is taken out of the cut search. One whose value a
// read or a successful cas saw took effect before that op returned, so it returns there (values are unique per key;
// a shared value keeps it pending). One nobody saw is pending: it may take effect at any time after its call, or
// never. A segment is checked without the pending writes first, which can only be stricter. If that is Illegal, it
// is checked again with every pending write called by its end; Illegal then is Illegal. Ok then holds only if no
// pending write was also lent to an earlier segment, because each takes effect at most once; otherwise Unknown.
func segmented(all []porcupine.Operation, target int, timeout time.Duration) (porcupine.CheckResult, int) {
	worst, segs := porcupine.Ok, 0
	for _, ops := range model.Partition(all) {
		res, n := segmentedKey(ops, target, timeout)
		segs += n
		if res == porcupine.Illegal || (res == porcupine.Unknown && worst == porcupine.Ok) {
			worst = res
		}
	}
	return worst, segs
}

func segmentedKey(all []porcupine.Operation, target int, timeout time.Duration) (porcupine.CheckResult, int) {
	key := all[0].Input.(Op).Key
	writers := map[string]int{}
	seenBy := map[string]int64{} // value -> earliest return of an op that saw it
	for _, o := range all {
		in, out := o.Input.(Op), o.Output.(output)
		if in.Kind != "get" {
			writers[in.Value]++
		}
		var v *string
		switch {
		case out.unknown:
		case in.Kind == "get":
			v = out.value
		case in.Kind == "cas" && out.ok:
			v = in.Expect
		}
		if v == nil {
			continue
		}
		if t, ok := seenBy[*v]; !ok || o.Return < t {
			seenBy[*v] = o.Return
		}
	}
	var ops, pending []porcupine.Operation
	for _, o := range all {
		if o.Output.(output).unknown {
			v := o.Input.(Op).Value
			t, seen := seenBy[v]
			if !seen || writers[v] != 1 {
				pending = append(pending, o)
				continue
			}
			o.Return, o.Output = max(t, o.Call), output{ok: true}
		}
		ops = append(ops, o)
	}
	sort.SliceStable(ops, func(i, j int) bool { return ops[i].Call < ops[j].Call })
	lent := make([]bool, len(pending))
	var state *string
	start, segs, worst := 0, 0, porcupine.Ok
	var maxRet int64 = math.MinInt64
	flush := func(end int) {
		seg := ops[start:end]
		init := state
		m := model
		m.Init = func() interface{} { return init }
		res := porcupine.CheckOperationsTimeout(m, seg, timeout)
		segs++
		if res == porcupine.Illegal {
			var segEnd int64 = math.MinInt64
			for _, o := range seg {
				segEnd = max(segEnd, o.Return)
			}
			var lend []int
			with := append([]porcupine.Operation(nil), seg...)
			for j, p := range pending {
				if p.Call <= segEnd {
					lend = append(lend, j)
					with = append(with, p)
				}
			}
			if len(lend) > 0 {
				res = porcupine.CheckOperationsTimeout(m, with, timeout)
			}
			for _, j := range lend {
				if res == porcupine.Ok && lent[j] {
					res = porcupine.Unknown
				}
				lent[j] = true
			}
		}
		if res != porcupine.Ok {
			fmt.Fprintf(os.Stderr, "%s segment %d (ops %d..%d, calls %d..%d): %s\n", key, segs, start, end, seg[0].Call, seg[len(seg)-1].Call, res)
		}
		if res == porcupine.Illegal || (res == porcupine.Unknown && worst == porcupine.Ok) {
			worst = res
		}
	}
	for i := range ops {
		o := ops[i]
		isolated := maxRet < o.Call && (i+1 == len(ops) || ops[i+1].Call > o.Return)
		if o.Return > maxRet {
			maxRet = o.Return
		}
		if !isolated || i+1-start < target {
			continue
		}
		in, out := o.Input.(Op), o.Output.(output)
		var next *string
		switch {
		case in.Kind == "get":
			next = out.value
		case in.Kind == "put", in.Kind == "cas" && out.ok:
			v := in.Value
			next = &v
		default:
			continue // a failed cas leaves a state it does not name
		}
		flush(i + 1)
		state, start = next, i+1
	}
	if start < len(ops) {
		flush(len(ops))
	}
	return worst, segs
}
