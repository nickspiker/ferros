(module
  (import "ferros" "syscall" (func $sys (param i64 i64 i64 i64 i64 i64 i64) (result i64)))
  (import "ferros" "arc_offset" (func $off (param i32) (result i64)))
  (memory 1)
  (data (i32.const 64) "hello from toka\n")
  ;; The console is grant slot 0: its token at the grants offset + G#204 (ferros_sys `cap`); a write is syscall 1 (call) with the message and a reply buffer as arc offsets.
  (func (export "_ferros_main") (param $grants i64) (param $input i64) (param $len i64)
    (drop (call $sys (i64.const 1) (i64.add (local.get $grants) (i64.const 0x204))
      (call $off (i32.const 64)) (i64.const 16) (call $off (i32.const 128)) (i64.const 16) (i64.const 0))))
)
