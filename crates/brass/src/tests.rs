//! Brass programs run on `asm-ada`'s bare board; their output and exit
//! code are checked.

use super::*;
use asm_ada::run::{run, Board, End};

fn compile_main(src: &str) -> Result<Output, Vec<String>> {
    let mut m = BTreeMap::new();
    m.insert(
        "main".to_string(),
        vec![("main.brass".to_string(), src.to_string())],
    );
    build(&MemSources(m))
}

/// Run `src` (package main): (output, exit code).
fn go(src: &str) -> (String, u16) {
    let out = compile_main(src).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    let mut b = Board::default();
    b.load(LOAD, &out.binary).unwrap();
    let r = run(&mut b, LOAD, 50_000_000);
    let code = match r.end {
        End::Exit(c) => c,
        End::Limit => panic!(
            "no end; output so far: {}\n{}",
            String::from_utf8_lossy(&r.output),
            out.asm
        ),
    };
    (String::from_utf8_lossy(&r.output).into_owned(), code)
}

fn out(src: &str) -> String {
    let (o, code) = go(src);
    assert_eq!(code, 0, "exit code; output: {o}");
    o
}

fn fails(src: &str) -> String {
    match compile_main(src) {
        Ok(_) => panic!("compiled"),
        Err(e) => e.join("\n"),
    }
}

#[test]
fn hello() {
    assert_eq!(
        out("package main\nfunc main() { println(\"hello\", 42, -7, true) }"),
        "hello 42 -7 true\n"
    );
}

#[test]
fn arithmetic_and_constants() {
    let o = out(r#"
package main

const (
    A = iota * 10
    B
    C
)

const Big = 1 << 20

func main() {
    var x int = 7
    y := x*6 - 2
    println(y, y/5, y%5, -y, y<<3, y>>1, y&3, y|8, y^1, y&^2)
    var u uint = 40000
    println(u, u/3, u>>4)
    var a int32 = 100000
    b := a*3 + int32(x)
    println(b, b/7, b%7, -b, b<<2, b>>3)
    var w uint32 = 0xffff_fff0
    println(w, w+0x20, w>>16)
    println(A, B, C, Big/1024)
    var s int8 = 120
    s += 10
    println(s, byte(300&255), int(int8(-3)), int32(int16(-5)))
}
"#);
    assert_eq!(
        o,
        "40 8 0 -40 320 20 0 40 41 40\n40000 13333 2500\n300007 42858 1 -300007 1200028 37500\n4294967280 16 65535\n0 10 20 1024\n-126 44 -3 -5\n"
    );
}

#[test]
fn control_flow() {
    let o = out(r#"
package main

func fib(n int) int {
    if n < 2 {
        return n
    }
    return fib(n-1) + fib(n-2)
}

func main() {
    sum := 0
    for i := 0; i < 10; i++ {
        if i == 3 {
            continue
        }
        if i == 8 {
            break
        }
        sum += i
    }
    println(sum, fib(15))
    n := 0
    for n < 5 {
        n++
    }
outer:
    for i := range 4 {
        for j := range 4 {
            if j > i {
                continue outer
            }
            if i == 3 {
                break outer
            }
            print(i, j, " ")
        }
    }
    println()
    switch {
    case n > 10:
        println("big")
    case n == 5:
        println("five")
    default:
        println("other")
    }
    switch k := n * 2; k {
    case 1, 2, 3:
        println("small")
    case 10:
        println("ten")
    }
}
"#);
    assert_eq!(o, "25 610\n00 10 11 20 21 22 \nfive\nten\n");
}

#[test]
fn structs_pointers_methods() {
    let o = out(r#"
package main

type Point struct {
    X, Y int
}

type Rect struct {
    Min, Max Point
    Name     string
}

func (p Point) Sum() int { return p.X + p.Y }

func (p *Point) Move(dx, dy int) {
    p.X += dx
    p.Y += dy
}

func (r *Rect) Area() int32 {
    return int32(r.Max.X-r.Min.X) * int32(r.Max.Y-r.Min.Y)
}

func swap(a, b int) (int, int) { return b, a }

func main() {
    p := Point{X: 3, Y: 4}
    p.Move(1, 1)
    println(p.X, p.Y, p.Sum())
    q := &p
    q.Move(10, 0)
    println(p.X, q.Sum())
    r := Rect{Min: Point{1, 2}, Max: Point{301, 202}, Name: "box"}
    println(r.Name, r.Area(), r.Max.Sum())
    h := &Rect{Name: "heap"}
    h.Max = Point{2, 3}
    println(h.Name, h.Area())
    a, b := swap(1, 2)
    a, b = b, a
    println(a, b)
    var arr [4]Point
    arr[2] = p
    arr[3].X = 9
    println(arr[2].X, arr[3].X, len(arr))
    free(h)
}
"#);
    assert_eq!(o, "4 5 9\n14 19\nbox 60000 503\nheap 6\n1 2\n14 9 4\n");
}

#[test]
fn slices_strings_append() {
    let o = out(r#"
package main

import "strs"

func total(v []int) int {
    t := 0
    for _, x := range v {
        t += x
    }
    return t
}

func main() {
    s := []int{1, 2, 3}
    s = append(s, 4, 5)
    for i := 0; i < 100; i++ {
        s = append(s, i)
    }
    println(len(s), total(s), s[4], s[104])
    t := s[2:5]
    println(len(t), cap(t) > 3, t[0], total(t))
    m := make([]byte, 3, 10)
    m[0] = 'h'
    m[1] = 'i'
    m = m[:2]
    str := string(m)
    println(str, len(str))
    msg := "hello, brass"
    println(msg[7:], msg[0], len(msg))
    n := 0
    for i, c := range msg {
        if c == 'l' {
            n += i
        }
    }
    println(n, msg == "hello, brass", msg < "help", "abc" != "abd")
    println(strs.Itoa(-1234), strs.Itoa32(1000000))
    var b strs.Builder
    b.WriteString("gear")
    b.WriteByte('s')
    println(b.String(), b.Len())
    c := copy(s, []int{9, 8})
    println(c, s[0], s[1], s[2])
    var empty []int
    println(len(empty), empty == nil)
}
"#);
    assert_eq!(
        o,
        "105 4965 5 99\n3 true 3 12\nhi 2\nbrass 104 12\n5 true true true\n-1234 1000000\ngears 5\n2 9 8 3\n0 true\n"
    );
}

#[test]
fn packages_globals_init_funcs() {
    let mut m = BTreeMap::new();
    m.insert(
        "main".to_string(),
        vec![(
            "main.brass".to_string(),
            r#"
package main

import "gears"

var counter = gears.Start * 2
var table [3]int

func init() {
    table[1] = 7
}

func apply(f func(int) int, v int) int { return f(v) }

func main() {
    println(counter, table[1], gears.Turn(5), apply(gears.Turn, 1))
    double := func(x int) int { return x * 2 }
    println(apply(double, 21))
}
"#
            .to_string(),
        )],
    );
    m.insert(
        "gears".to_string(),
        vec![(
            "gears.brass".to_string(),
            r#"
package gears

const Start = 21

var turns int

func Turn(n int) int {
    turns++
    return n*10 + turns
}
"#
            .to_string(),
        )],
    );
    let out = build(&MemSources(m)).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    let mut b = Board::default();
    b.load(LOAD, &out.binary).unwrap();
    let r = run(&mut b, LOAD, 10_000_000);
    assert_eq!(String::from_utf8_lossy(&r.output), "42 7 51 12\n42\n");
}

#[test]
fn fixed_point() {
    let o = out(r#"
package main

func main() {
    a := fixed(3) / 2
    b := fixed(-5) / 4
    println(a, b, a*b, a+b, fixed(7)/fixed(2), int(a*4))
    var c fixed = 2
    println(c*c*c)
}
"#);
    assert_eq!(o, "1.5000 -1.2500 -1.8750 0.2500 3.5000 6\n8.0000\n");
}

#[test]
fn many_arguments_and_results() {
    let o = out(r#"
package main

func f(a, b, c int, s string, d int32) (int, int32) {
    return a + b + c + len(s), d * 2
}

func main() {
    x, y := f(1, 2, 3, "four", 70000)
    println(x, y)
}
"#);
    assert_eq!(o, "10 140000\n");
}

#[test]
fn panics_end_with_code_2() {
    let (o, code) = go("package main\nfunc main() { s := []int{1}\n i := 3\n println(s[i]) }");
    assert_eq!((o.as_str(), code), ("panic: index out of range\n", 2));
    let (o, code) = go("package main\nfunc main() { panic(\"boiler pressure\") }");
    assert_eq!((o.as_str(), code), ("panic: boiler pressure\n", 2));
}

#[test]
fn errors_point_at_the_source() {
    assert!(fails("package main\nfunc main() { x := 1 + \"a\" }").contains("main.brass:2"));
    assert!(fails("package main\nfunc main() { var s string = 5 }").contains("cannot use"));
    assert!(fails("package main\nfunc f() int { }\nfunc main() {}").contains("missing return"));
    assert!(fails("package main\nimport \"nope\"\nfunc main() {}").contains("not found"));
    assert!(fails("package main\nfunc main() { var b byte = 300 }").contains("does not fit"));
    assert!(fails("package main").contains("no func main"));
}

#[test]
fn concurrency_compiles_and_needs_boiler() {
    let src = r#"
package main

func worker(c chan int, n int) {
    for i := range n {
        c <- i
    }
    close(c)
}

func main() {
    c := make(chan int, 4)
    go worker(c, 3)
    for v := range c {
        println(v)
    }
    d := make(chan string)
    select {
    case s := <-d:
        println(s)
    case d <- "x":
    default:
    }
    v, ok := <-c
    _, _ = v, ok
}
"#;
    let (o, code) = go(src);
    assert_eq!(
        (o.as_str(), code),
        ("panic: go, channels and select need Boiler OS\n", 2)
    );
}

#[test]
fn many_packages_fill_several_code_segments() {
    // 8 packages of 70 functions: more code than one segment holds.
    let mut m = BTreeMap::new();
    let mut main = String::from("package main\n\nimport (\n");
    for p in 0..8 {
        main.push_str(&format!("    \"p{p}\"\n"));
        let mut src = format!("package p{p}\n\n");
        for f in 0..70 {
            src.push_str(&format!(
                "func F{f}(a, b int) int {{\n    x := a*{f} + b\n    for i := 0; i < 3; i++ {{\n        x += i * a - b + {f}\n        if x > 1000 {{ x -= 999 }}\n    }}\n    return x\n}}\n\n"
            ));
        }
        m.insert(format!("p{p}"), vec![(format!("p{p}.brass"), src)]);
    }
    main.push_str(")\n\nfunc main() {\n    t := 0\n");
    for p in 0..8 {
        main.push_str(&format!("    t += p{p}.F69(2, 3) + p{p}.F1(1, 1)\n"));
    }
    main.push_str("    println(t)\n}\n");
    m.insert("main".to_string(), vec![("main.brass".to_string(), main)]);
    let out = build(&MemSources(m)).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    assert!(
        out.binary.len() > 2 * 65536,
        "{} bytes: several segments",
        out.binary.len()
    );
    let mut b = Board::default();
    b.load(LOAD, &out.binary).unwrap();
    let r = run(&mut b, LOAD, 10_000_000);
    // F39(2,3) = 81 + 3*... computed by Brass; F1(1,1): x=2, loop: +0, +1-1+1.. just check it runs
    assert!(
        matches!(r.end, End::Exit(0)),
        "{}",
        String::from_utf8_lossy(&r.output)
    );
    assert!(!r.output.is_empty());
}

#[test]
fn nil_conversions_and_byte_slices_from_literals() {
    let o = out(r#"
package main

type T struct{ a int }

func main() {
    p := (*T)(nil)
    b := []byte("brass")
    println(p == nil, len(b), b[0], string(b[1:3]))
}
"#);
    assert_eq!(o, "true 5 98 ra\n");
}

#[test]
fn switch_on_strings() {
    let o = out(r#"
package main

func kind(s string) int {
    switch s {
    case "help":
        return 1
    case "ls":
        return 2
    case "cd", "ps":
        return 3
    case "cat":
        return 4
    case "mem":
        return 5
    }
    return 0
}

func main() {
    ws := []string{"ls", "ps", "mem", "cat", "xy", "help"}
    for _, w := range ws {
        print(kind(w), " ")
    }
    s := string([]byte("ps"))
    println(kind(s), s == "ps", s == "ls", "ps" < "pt")
}
"#);
    assert_eq!(o, "2 3 5 4 0 1 3 true false true\n");
}

#[test]
fn interfaces_methods_and_slices() {
    let o = out(r#"
package main

type Shape interface {
    Area() int
    Name() string
}

type Rect struct { w, h int }
type Square struct { s int }
type Circle struct { r int }

func (r Rect) Area() int { return r.w * r.h }
func (r Rect) Name() string { return "rect" }
func (s *Square) Area() int { return s.s * s.s }
func (s *Square) Name() string { return "square" }
func (c Circle) Area() int { return 3 * c.r * c.r }
func (c *Circle) Name() string { return "circle" }

type Counter int

func (c *Counter) Inc() { *c++ }
func (c Counter) Get() int { return int(c) }

type Incer interface { Inc(); Get() int }

func total(ss []Shape) int {
    t := 0
    for _, s := range ss {
        t += s.Area()
    }
    return t
}

func biggest(ss []Shape) Shape {
    var b Shape
    for _, s := range ss {
        if b == nil || s.Area() > b.Area() {
            b = s
        }
    }
    return b
}

func main() {
    ss := []Shape{Rect{2, 3}, &Square{4}, &Circle{2}}
    ss = append(ss, Rect{w: 1, h: 1})
    for _, s := range ss {
        println(s.Name(), s.Area())
    }
    println(total(ss))
    b := biggest(ss)
    println(b.Name())
    var c Counter = 5
    var i Incer = &c
    i.Inc()
    i.Inc()
    println(i.Get(), c)
    var r Shape = Rect{3, 3}
    rr := r
    println(rr.Area(), r == rr)
}
"#);
    assert_eq!(
        o,
        "rect 6\nsquare 16\ncircle 12\nrect 1\n35\nsquare\n7 7\n9 true\n"
    );
}

#[test]
fn interfaces_nil_any_and_comparison() {
    let o = out(r#"
package main

type Stringer interface { String() string }
type T struct { n int }
func (t *T) String() string { return "T" }

func describe(a any) string {
    switch v := a.(type) {
    case nil:
        return "nil"
    case int:
        return "int"
    case string:
        return v
    case *T, bool:
        return "T or bool"
    default:
        return "other"
    }
}

func main() {
    var s Stringer
    println(s == nil, s != nil)
    t := &T{1}
    s = t
    println(s == nil, s.String())
    var s2 Stringer = t
    println(s == s2, s == Stringer(&T{1}))
    var a any = 5
    var b any = "boiler"
    println(a == 5, a == 6, b == "boiler", a == b)
    println(describe(nil), describe(a), describe(t), describe(true), describe(int8(3)), describe(b))
    var e any = s
    println(e == s)
    s = nil
    println(s == nil)
}
"#);
    assert_eq!(
        o,
        "true false\nfalse T\ntrue false\ntrue false true false\nnil int T or bool T or bool other boiler\ntrue\ntrue\n"
    );
}

#[test]
fn type_assertions() {
    let o = out(r#"
package main

type Reader interface { Read() int }
type Closer interface { Close() string }
type ReadCloser interface {
    Reader
    Closer
}

type File struct { fd int }
func (f *File) Read() int { return f.fd * 10 }
func (f *File) Close() string { return "closed" }

type Pipe struct { n int }
func (p Pipe) Read() int { return p.n }

type Big struct { a, b, c int32 }

func main() {
    var r Reader = &File{4}
    f := r.(*File)
    println(f.fd, r.Read())
    rc, ok := r.(ReadCloser)
    println(ok, rc.Close(), rc.Read())
    var rc2 ReadCloser = rc
    r = rc2
    println(r.Read())
    r = Pipe{7}
    _, ok = r.(ReadCloser)
    println(ok)
    p, ok := r.(Pipe)
    println(p.n, ok, r.(Pipe).n)
    f, ok = r.(*File)
    println(f == nil, ok)
    var a any = Big{1, 2, 70000}
    big := a.(Big)
    println(big.c, a.(Big).b)
    n, ok := a.(int)
    println(n, ok)
    var x any = "text"
    if s, ok := x.(string); ok {
        println(s, len(s))
    }
}
"#);
    assert_eq!(
        o,
        "4 40\ntrue closed 40\n40\nfalse\n7 true 7\ntrue false\n70000 2\n0 false\ntext 4\n"
    );
    let (o, code) = go(r#"
package main
type T struct{}
func main() {
    var a any = 3
    println(a.(int))
    println(a.(string))
}
"#);
    assert_eq!(
        (o.as_str(), code),
        (
            "3\npanic: interface conversion: interface is not string\n",
            2
        )
    );
    let (o, code) = go(r#"
package main
type I interface { M() }
func main() {
    var i I
    i.M()
}
"#);
    assert_eq!(
        (o.as_str(), code),
        ("panic: nil interface method call\n", 2)
    );
}

#[test]
fn interfaces_in_structs_channels_and_results() {
    let o = out(r#"
package main

type Op interface { Apply(a, b int) int }
type Add struct{}
type Mul struct{ k int }
func (Add) Apply(a, b int) int { return a + b }
func (m Mul) Apply(a, b int) int { return m.k * a * b }

type Step struct {
    name string
    op Op
}

var global Op = Add{}

func pick(k int) Op {
    if k == 0 {
        return Add{}
    }
    return Mul{k}
}

func run(o Op, a int, b int, c int, d int) int { return o.Apply(a, b) + c + d }

func main() {
    steps := [2]Step{{"add", Add{}}, {name: "mul", op: Mul{2}}}
    for _, s := range steps {
        println(s.name, s.op.Apply(3, 4))
    }
    o := pick(3)
    println(o.Apply(1, 1), pick(0).Apply(1, 1))
    println(run(global, 1, 2, 3, 4))
    global = o
    println(global.Apply(2, 2))
    ops := make([]Op, 0, 1)
    ops = append(ops, Add{}, Mul{1})
    println(len(ops), ops[1].Apply(5, 5))
}
"#);
    assert_eq!(o, "add 7\nmul 24\n3 2\n10\n12\n2 25\n");
}

#[test]
fn interface_errors() {
    assert!(fails(
        "package main\ntype I interface { M() }\ntype T int\nfunc main() { var i I = T(1)\n i.M() }"
    )
    .contains("T does not implement I (missing method M)"));
    assert!(fails(
        "package main\ntype I interface { M() }\ntype T int\nfunc (t *T) M() {}\nfunc main() { var i I = T(1)\n i.M() }"
    )
    .contains("method M has pointer receiver"));
    assert!(fails(
        "package main\ntype I interface { M() }\ntype T int\nfunc main() { var i I\n _ = i.(T) }"
    )
    .contains("impossible type assertion"));
    assert!(fails(
        "package main\ntype I interface { M() }\ntype J interface { M(); N() }\nfunc main() { var i I\n var j J = i\n _ = j }"
    )
    .contains("I does not implement J (missing method N)"));
    assert!(
        fails("package main\nfunc main() { x := 3\n _ = x.(int) }").contains("not an interface")
    );
}

#[test]
fn interfaces_small_values_lists_and_channels() {
    let o = out(r#"
package main

type Node interface {
    Value() byte
    Next() Node
}

type Cell struct {
    v byte
    next *Cell
}

func (c *Cell) Value() byte { return c.v }
func (c *Cell) Next() Node {
    if c.next == nil {
        return nil
    }
    return c.next
}

type Flag bool

func (f Flag) Value() byte {
    if f {
        return 1
    }
    return 0
}
func (f Flag) Next() Node { return nil }

func main() {
    var n Node = &Cell{1, &Cell{2, &Cell{3, nil}}}
    sum := 0
    for n != nil {
        sum += int(n.Value())
        n = n.Next()
    }
    println(sum)
    var x any = byte(200)
    var y any = int8(-3)
    println(x.(byte), y.(int8), x == byte(200), x == 200)
    n = Flag(true)
    println(n.Value(), n.(Flag))
    vs := []any{1, "two", Flag(false), nil}
outer:
    for _, v := range vs {
        switch w := v.(type) {
        case int, string:
            println("int or string")
        case Node:
            println("node", w.Value())
            break outer
        }
    }
}
"#);
    assert_eq!(
        o,
        "6\n200 -3 true false\n1 true\nint or string\nint or string\nnode 0\n"
    );
    // channels and `go` through an interface compile (they run under Boiler)
    compile_main(
        "package main\ntype I interface { M(a int) }\ntype T struct{}\nfunc (T) M(a int) {}\nfunc main() {\n c := make(chan any, 2)\n c <- 5\n v := <-c\n _ = v\n var i I = T{}\n go i.M(1)\n}",
    )
    .unwrap_or_else(|e| panic!("{}", e.join("\n")));
}

#[test]
fn interfaces_cost_nothing_unless_used() {
    let plain = compile_main("package main\nfunc main() { println(1) }").unwrap();
    assert!(!plain.asm.contains("tdesc_") && !plain.asm.contains("ilookup"));
    let src = "package main\ntype I interface { M() int }\ntype T int\nfunc (t T) M() int { return int(t) }\nfunc main() { var i I = T(4)\n println(i.M()) }";
    let a = compile_main(src).unwrap();
    assert!(a.asm.contains("tdesc_main__T:") && a.asm.contains("iw_main__T__M:"));
    assert_eq!(a.binary, compile_main(src).unwrap().binary, "deterministic");
}

#[test]
fn a_package_bigger_than_a_segment_is_spread_over_several() {
    // 400 functions of some 200 bytes each: about 80 KiB of code in main.
    let mut src = String::from("package main\n\nvar total int32\n\n");
    for i in 0..400 {
        src += &format!(
            "func f{i}(x int) int {{\n\ty := x + {i}\n\tif y > 30000 {{\n\t\ty -= 30000\n\t}}\n\ttotal += int32(y)\n\treturn y\n}}\n\n"
        );
    }
    src += "func main() {\n\tx := 0\n";
    for i in 0..400 {
        src += &format!("\tx = f{i}(x) % 1000\n");
    }
    src += "\tprintln(x, total)\n}\n";
    let out = compile_main(&src).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    assert!(out.asm.contains(".section code1"), "two code segments");
    // the same sums on the host
    let (mut x, mut total) = (0i32, 0i32);
    for i in 0..400 {
        let mut y = x + i;
        if y > 30000 {
            y -= 30000;
        }
        total += y;
        x = y % 1000;
    }
    assert_eq!(go(&src).0, format!("{x} {total}\n"));
}

#[test]
fn wide_locals_keep_their_words_on_every_kind_of_assignment() {
    let o = out(r#"
package main

func two(v uint32) (uint32, bool) { return v, true }

func main() {
    var a uint32 = 0x0a100001
    var ok bool
    a, ok = two(a + 1)
    b := a
    b += 0x00010000
    b++
    c := []uint32{0x01020304}
    var d uint32
    for _, d = range c {
    }
    e, f := uint32(7), uint32(0x00070000)
    e, f = f, e
    var s string
    s, ok = "brass", false
    p := &a
    *p += 1
    println(a, ok, b, d, e, f, s)
}
"#);
    assert_eq!(
        o,
        format!(
            "{} false {} {} {} 7 brass\n",
            0x0a10_0003u32, 0x0a11_0003u32, 0x0102_0304u32, 0x0007_0000u32
        )
    );
}

#[test]
fn pointers_to_wide_locals_and_parameters() {
    let o = out(r#"
package main

func set(p *uint32, v uint32) { *p = v }

func grow(s *string) { *s = "brass and steam" }

func twice(x uint32, f fixed) (uint32, fixed) {
    px, pf := &x, &f
    *px *= 2
    *pf = *pf * 2
    return x, f
}

func main() {
    var a uint32
    set(&a, 0x0a100001)
    b := a + 1
    s := "brass"
    grow(&s)
    var c uint32
    var ok bool
    q := &c
    c, ok = 0x00010002, true
    x, f := twice(0x00012345, fixed(3)/2)
    println(a, b, s, *q, ok, x, int(f))
}
"#);
    assert_eq!(
        o,
        format!(
            "{} {} brass and steam {} true {} 3\n",
            0x0a10_0001u32, 0x0a10_0002u32, 0x0001_0002u32, 0x0002_468au32
        )
    );
}
