# Brass — the Logophor 16's programming language

Brass is the high-level language of the Logophor 16: Go's syntax and
spirit, cut to a 16-bit machine without a garbage collector. Boiler,
Panorama and their applications are written in it. This document is the
language's single source of truth, as `SPEC.md` is the machine's;
decisions taken here are marked **[B]** and listed in section 14.

Status: accepted (2026-11-06); the compiler `brass` (crate `brass`, a
host tool in Rust) implements it as section 15 lists.

## 1. In one paragraph

A Brass program is a set of packages, each a folder of `.brass` files,
compiled together into Ada assembly for `ada-asm` and linked into one
binary. It has Go's declarations, statements and expressions, its
packages and export rule (capitalised names), multiple results, methods,
slices and strings. It has no garbage collector: memory comes from `new`
and `make` and goes back with `free`. Integers are 16 bits wide by
default (`int`), as the CPU's registers are; pointers are 32-bit linear
addresses. Under Boiler, `go` starts a task and channels are Boiler's
message ports.

## 2. Source and packages

- Source is UTF-8; string and character literals are stored as ISO
  8859-1 (the machine's character set); characters outside it are an
  error **[B]**.
- Every file starts with `package name`. A package is a folder; its
  import path is the folder's path from the program's root (`import
  "panorama/widgets"`) or a library package of the compiler's own
  library folder (`import "console"`).
- Names starting with a capital letter are exported, as in Go.
- `package main` with `func main()` is the program. Before `main` the
  package-level variables are initialised in dependency order, then each
  package's `func init()`, as in Go.
- There are no import cycles.

## 3. Lexical elements

As Go: identifiers (ASCII letters, digits, `_`), keywords, operators,
automatic semicolons at line ends, `//` and `/* */` comments.

Keywords: `break case chan const continue default else for func go if
import interface package range return select struct switch type var`.
(Go's `defer`, `fallthrough`, `goto` and `map` are not in Brass v1 **[B]**.)

Literals: integers in decimal, `0x`, `0o`, `0b` (with `_` separators);
characters `'a'`, `'\n'`, `'\x41'` (type `byte` when untyped constants
need one); strings `"…"` with Go's escapes and raw strings `` `…` ``.
No floating-point literals **[B]** (there is no floating-point type).

## 4. Types

| Type | Size | Notes |
| --- | --- | --- |
| `bool` | 1 byte | `true`, `false` |
| `int8`, `uint8` (= `byte`) | 1 | |
| `int16`, `uint16` | 2 | |
| `int32`, `uint32` | 4 | in register pairs |
| `int`, `uint` | 2 | = `int16`, `uint16`: the machine's word **[B]** |
| `uintptr` | 4 | a linear address (segment × 65 536 + offset) |
| `fixed` | 4 | 16.16 fixed point, as the Stereoscope uses **[B]** |
| `*T` | 4 | a linear address; `nil` is 0 |
| `[N]T` | N × size(T) | arrays are values |
| `[]T` | 8 | slice: pointer, length (16 bits), capacity (16 bits) |
| `string` | 6 | pointer, length (16 bits); immutable bytes |
| `struct { … }` | fields in order, words on even addresses | |
| `func(…) …` | 4 | the function's linear address |
| `interface { … }` | 8 | a pointer to the value, a pointer to the dynamic type's descriptor (method table) |
| `chan T` | 4 | a Boiler message port (under Boiler only) |

- Named types, type conversions, untyped constants and the conversion
  rules are Go's. Integer arithmetic wraps; division by zero raises the
  CPU's exception (a panic).
- `fixed` takes `+ - * /` and comparisons; conversions `fixed(i)` from
  an integer and `int(f)` (truncated) to one; constants like `fixed(3) /
  2` are folded **[B]**.
- Slices and strings index with `int` (16 bits): a slice holds at most
  32 767 elements **[B]**. Bigger memory is reached through pointers and
  `uintptr` arithmetic.
- Index and slice bounds are checked; a failed check panics.
- Interfaces are Go's: method sets, implicit implementation, type
  assertions `x.(T)` and type switches; the empty interface `any` holds
  any value (the value is copied to the heap when it is not a pointer)
  **[B]**.

## 5. Memory

There is no garbage collector **[B]**:

- `new(T)` and `make([]T, n[, cap])`, `make(chan T, n)` allocate from the
  heap (the runtime's allocator; Boiler's under Boiler); `free(p)` and
  `free(s)` give a pointer's or a slice's memory back. Using freed memory
  is undefined, as in C.
- `append` grows a slice in place while its capacity lasts, else it
  allocates a larger array, copies, and frees the old one **[B]**: keep
  using only the slice `append` returns.
- Composite literals `&T{…}` allocate; `T{…}` and `[N]T{…}` are values.
- String literals live in the program; `+` on strings is allowed only on
  constants. Building strings goes through the `strs` package (a builder
  over a byte slice) **[B]**.
- Local variables live on the stack, also when their address is taken: a
  pointer to a local must not outlive the call (no escape analysis)
  **[B]**.
- Function literals may not capture local variables (no closures over the
  stack) **[B]**; they may use package-level variables and their own
  parameters.

## 6. Declarations

Go's: `const` (with `iota`), `var` (also `x := e` and `a, b := f()`),
`type` (named types, struct and interface types, aliases with `=`),
`func` (functions and methods with value or pointer receivers). A
function declared without a body is implemented in assembly (an `.s`
file in the package's folder) under the symbol `package.Name` **[B]**.

## 7. Expressions and statements

- Expressions: Go's operators and precedence (`+ - * / % & | ^ &^ << >>`,
  comparisons, `&& || !`, unary `- ^ & *`, `<-`), calls, method calls,
  index, slice `a[i:j]`, conversions, composite literals, `x.(T)`.
- Statements: assignments (`=`, `op=`, tuples), `++`/`--`, `if` with an
  init statement, `for` (condition, three clauses, `range` over arrays,
  slices, strings (bytes), channels and integers `for i := range 10`),
  `switch` with or without a tag and type switches (no `fallthrough`),
  `select` over channels (under Boiler), `break` and `continue` with
  labels, `return`, `go`.
- Evaluation order and the short-circuit rules are Go's.

## 8. Built-in functions

`len`, `cap`, `new`, `make`, `append`, `copy`, `free`, `panic`,
`print` and `println` (to the console: TRAP #4 on a bare machine,
Boiler's console in a task), `min`, `max`. A panic prints its message and
ends the task (on a bare machine: the emergency monitor) **[B]**.

## 9. Concurrency (under Boiler)

`go f(x)` starts a Boiler task running `f(x)` (its own stack; arguments
are copied). `make(chan T, n)` makes a message port with room for `n`
values (0: unbuffered, the sender waits for the receiver); `c <- v`,
`v := <-c`, `v, ok := <-c`, `close(c)` and `select` are Go's. On a bare
machine (no Boiler) `go` and channel operations panic **[B]**.

## 10. Hardware access

- Conversions between `uintptr` and pointers are allowed:
  `(*uint16)(uintptr(0xffff_4280))` is Telegraph's INTREQ. Every access
  through a pointer is carried out as written (the compiler keeps no
  memory values in registers across statements), so device registers
  need no `volatile` **[B]**.
- The package `hw` names the chips' registers and gives `Peek`/`Poke`
  helpers; `asm` functions (section 6) do the rest, e.g. interrupt
  handlers.

## 11. Calling convention and code

- Arguments go in R1–R4 in order (a 32-bit value takes two registers,
  high word first; a slice, string or interface its words), the rest on
  the stack; results in R1, R2 … the same way (more than four words: in
  memory the caller provides). R5–R9 are kept by the callee, R1–R4 and
  R10/R11 are not, R12 is the stack pointer in S2 (SPEC.md 11) **[B]**.
- Every function is a far function (`CALLF`/`RETF`), so code may span
  segments; S1 holds the program's data segment **[B]**.
- The compiler writes Ada assembly; `ada-asm` assembles and links it with
  the runtime and the assembly parts. Output: a raw binary for 0040:0000
  (what Foundry boots and the program slot loads) and, from step 11b,
  Boiler's executable format.

## 12. The compiler

`brass build DIR -o prog.bin [--listing prog.lst] [--asm out.s]` compiles
the program in `DIR` (package `main`) with the packages it imports.
`brass run DIR` runs it on `ada-asm`'s bare board (debug console).
Errors carry file, line and column. The compiler checks types and
reports unused imports and variables as Go does.

## 13. Library (v1)

| Package | Content |
| --- | --- |
| `console` | text output and keys through the ROM's TRAPs |
| `hw` | register names of Babbage, Lumière, Calliope, Telegraph; peek/poke |
| `strs` | string building and conversion (`Itoa`, `Builder`) |
| `lumiere` | modes, palette, pixels, sprites |
| `blitter` | fills, copies, lines through Babbage's Blitter |
| `calliope` | channels, samples, notes |
| `sd` | blocks through TRAPs #5/#6 |

Boiler's packages (`boiler`, `files`, `net` …) and Panorama's (`panorama`)
come with steps 11b–11d.

## 14. Decision log

1. Brass after Go; the name is the owner's (2026-11-06).
2. `int` is 16 bits; `int32`/`uint32` in register pairs; pointers and `uintptr` 32-bit linear addresses.
3. No garbage collector: `new`/`make`/`free`; `append` frees the old array when it moves.
4. No floating point; `fixed` (16.16) instead.
5. Not in v1: `defer`, `fallthrough`, `goto`, `map`, closures over locals, string `+` at run time.
6. Slices and strings index with 16 bits (at most 32 767 elements).
7. Interfaces as in Go, with `any`.
8. Functions without a body are assembly (`package.Name`).
9. Every pointer access is carried out (device registers need no `volatile`).
10. Calling convention: SPEC.md's (R1–R4, R5–R9 kept), every function far (CALLF/RETF).
11. `go` and channels map to Boiler's tasks and message ports; on a bare machine they panic.
12. A panic prints and ends the task; on a bare machine it enters the monitor.
13. Code layout (v1): code in segment $0040 (at most 64 KiB), the data segment $0041 (S1), the heap from $0042 to the end of fast RAM (blocks of 2^n bytes with free lists), the stack at 003F:FFFE (S2); `main` returning writes 0 to DBGEXIT and enters the monitor (TRAP #0) with the output kept on the screen.
14. A function keeps R5–R9 (the compiler uses R9/R10 as scratch and saves R9), structs and arrays as parameters are passed as the address of a copy, results take at most four words (a struct is returned through a pointer).
15. Package variables are initialised in source order, packages in import order (dependencies first).
16. Interfaces (2026-11-06): a value is two far pointers, data then type descriptor; in registers R1:R2 data (segment, offset), R3:R4 descriptor, in memory each pointer offset first; nil is a descriptor of 0:0. A pointer value is the data pointer itself, any other value is copied to a new heap block. A descriptor `tdesc_…` (data segment, one per type converted to an interface or named in an assertion) holds the method count, the boxed value's size (0 for a pointer type) and per method a name number, offset and segment, sorted by name; method-name numbers are given program-wide in order of first use. Every entry takes the receiver as a pointer: value-receiver methods go through a compiler-made wrapper `func (p *T) M(…) { return (*p).M(…) }`; the descriptor of `*T` lists both kinds, the one of `T` the value methods (Go's method sets). Calls look the method up at run time (`rt_ifind`, linear); assertions to a concrete type compare descriptors, to an interface type look up each method. `==` is true for the same descriptor and either the same data pointer or, for a non-pointer type, the same bytes (a boxed string compares pointer and length). The runtime's interface routines and any descriptor are left out of a program without interfaces.

## 15. Implementation status (compiler v1, step 11a)

Built and tested (`crates/brass/src/tests.rs` on the bare board, and on
the machine from an SD card): packages and imports with assembly parts,
constants with `iota` and untyped constant arithmetic, all integer types
and `fixed`, pointers, arrays, slices, strings, structs, named types,
methods with value and pointer receivers, function values and literals
without captures, multiple results, `if`/`for`/`range`/`switch` with
labels, `break`, `continue`, the built-ins (`len cap new make append
copy free panic print println min max`), conversions (integers, `fixed`,
pointers ↔ `uintptr`, `[]byte` ↔ `string`), bounds and nil checks, the
library packages `console` and `strs`.

Since step 11b: `go`, channels (`make`, send, receive, `v, ok := <-c`,
`close`, `range`), `select` with `default` (BRASS.md 9; the runtime calls
Boiler through vectors, `compile/conc.rs` documents the calls), code in
several segments (packages are packed into 64 KiB segments, every segment
has its own copy of the runtime's code), `(*T)(nil)`, `[]byte("literal")`.

Since step 11d: a package bigger than a segment is spread over several at
its functions (every call is far; a single function must stay below 60
KiB); a local of several words (32-bit integers, `fixed`, pointers,
strings, slices, interfaces) whose address is taken lives in a cell in
memory order (low word first, as through any pointer) and its frame slot
holds the cell's address, while other locals keep register order in their
slots; `a, b = f()` into such locals and `switch` over strings fixed (the
runtime's string comparison read the descriptors' words swapped); a
`switch` with `default` whose cases all end in `return` ends a function.

Since 2026-11-06 (decision 16, `compile/iface.rs`): interfaces with
implicit implementation and Go's method sets, `any`, embedded interfaces,
calls through interfaces (pointer and value receivers), conversions in
assignments, arguments, results, composite literals, channels and
comparisons, explicit conversions `I(x)`, interface → interface
assignment, `x.(T)` and `v, ok := x.(T)` to concrete and interface types,
type switches with `nil`, `default`, several types per case and the
binding, `==`/`!=` between interfaces and against `nil` or a value; the
errors "T does not implement I (missing method M)", "method M has pointer
receiver", "impossible type assertion". An interface takes 4 result
words, so a function returns at most one interface (no `(I, bool)`).

Not yet (they come with the steps that need them): the packages `hw`,
`lumiere`, `blitter`, `calliope`, `sd` (with the programs that use them),
comparison of structs and arrays, method values (`x.M` without a call),
printing an interface with `print`.
