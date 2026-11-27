(module
  (import "ferros" "view" (memory $fonts 1))
  (func (export "scribble") (param i32) (i32.store $fonts (local.get 0) (i32.const 7))))
