	.file	"keyreg_probe.7cb9c0501c03fbed-cgu.0"
	.section	.text.keyreg_check_or_die,"ax",@progbits
	.globl	keyreg_check_or_die
	.p2align	2
	.type	keyreg_check_or_die,@function
keyreg_check_or_die:
	.cfi_startproc
	mov	x8, x0
	//APP
	msr	DAIFSet, #15
	ld1	{ v0.16b }, [x8]
	ld1	{ v1.16b }, [x1]
	eor	v2.16b, v0.16b, v1.16b
	mvn	v3.16b, v2.16b
	umaxv	b4, v3.16b
	mov	w0, v4.s[0]
	cbz	w0, .Ltmp0
	movi	v0.16b, #0
	movi	v1.16b, #0
	movi	v2.16b, #0
	movi	v3.16b, #0
.Ltmp1:
	wfi
	b	.Ltmp1
.Ltmp0:
	mov	x9, v0.d[0]
	//NO_APP
	mov	x0, x9
	ret
.Lfunc_end0:
	.size	keyreg_check_or_die, .Lfunc_end0-keyreg_check_or_die
	.cfi_endproc

	.ident	"rustc version 1.94.0 (4a4ef493e 2026-03-02)"
	.section	".note.GNU-stack","",@progbits
