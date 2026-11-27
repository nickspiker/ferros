(module
  (import "ferros" "share" (memory $ring 1))
  (import "ferros" "view" (memory $fonts 1))
  (memory $own 1)
  (func (export "copy_glyph") (param $at i32) (param $to i32) (result i32)
    ;; read a word of the view (fonts), write it into the shared ring and our own memory
    (i64.store $ring offset=16 (local.get $to) (i64.load $fonts offset=8 (local.get $at)))
    (i32.store $own (local.get $to) (i32.load8_u $fonts (local.get $at)))
    (v128.store $ring offset=4096 (local.get $to) (v128.load32_splat $fonts offset=100000 (local.get $at)))
    (f64.store $ring (local.get $to) (f64.load $fonts (i32.add (local.get $at) (local.get $to))))
    (i32.add (memory.size $fonts) (memory.size $ring)))
)
