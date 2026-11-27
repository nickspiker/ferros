// ferros-as: the masking assembler (docs/SANDBOX.md "The masking assembler"; the owner, 2026-11-26: "Full proper LLVM pass", C++ over a pure-Rust rewriter).
// It assembles AArch64 assembly with LLVM's own parser and encoder, and between the two it rewrites every instruction into the forms the install-time checker (ferros_sandbox) accepts.
// The rewrite works on LLVM's decoded instruction (opcode, operands, register classes), never on the text, so every source goes through the same place: rustc's emitted assembly with its inline asm!, clang's -S for C and C++, and hand-written .S files.
// The rules are SANDBOX.md's ("What the emitter rewrites"), the same tools/ferros-mask applied to the text until 2026-11-26, byte for byte; the checker stays the authority, and anything let through here is checked again at stow.
//
// Usage: ferros-as <in.s> -o <out.o> [-mattr=<features>] [--fixed K] [--bti]
//        ferros-as --dump <in.s> [-mattr=<features>]   (each instruction's opcode, flags and operands, for working on the rules)
//
// Register roles (reserved in codegen): x28 arc base, x27 mask, x26 masked offset (fixed: the address), x25 address in the arc, x24 scratch; x23 a handle's base, x22 its mask, x21 an offset masked by it (SANDBOX.md "Handles"; written by hand in ferros_sys and passed through).

#include "MCTargetDesc/AArch64TargetStreamer.h"
#include "llvm/BinaryFormat/ELF.h"
#include "llvm/MC/MCAsmBackend.h"
#include "llvm/MC/MCAsmInfo.h"
#include "llvm/MC/MCAssembler.h"
#include "llvm/MC/MCCodeEmitter.h"
#include "llvm/MC/MCContext.h"
#include "llvm/MC/MCELFStreamer.h"
#include "llvm/MC/MCInst.h"
#include "llvm/MC/MCInstPrinter.h"
#include "llvm/MC/MCInstrDesc.h"
#include "llvm/MC/MCInstrInfo.h"
#include "llvm/MC/MCObjectFileInfo.h"
#include "llvm/MC/MCObjectWriter.h"
#include "llvm/MC/MCParser/MCAsmParser.h"
#include "llvm/MC/MCParser/MCTargetAsmParser.h"
#include "llvm/MC/MCRegisterInfo.h"
#include "llvm/MC/MCSection.h"
#include "llvm/MC/MCStreamer.h"
#include "llvm/MC/MCSubtargetInfo.h"
#include "llvm/MC/MCSymbolELF.h"
#include "llvm/MC/MCTargetOptions.h"
#include "llvm/MC/TargetRegistry.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/MemoryBuffer.h"
#include "llvm/Support/SourceMgr.h"
#include "llvm/Support/raw_ostream.h"
#include "llvm/TargetParser/Triple.h"

#include <map>
#include <optional>
#include <string>
#include <vector>

using namespace llvm;

extern "C" void LLVMInitializeAArch64TargetInfo();
extern "C" void LLVMInitializeAArch64TargetMC();
extern "C" void LLVMInitializeAArch64AsmParser();

namespace {

/// The masking mode (SANDBOX.md "Modes"): ring `and x26, xN, x27` then `[x28, x26]` or x25; fixed(K) `bfi x26, xN, #0, #K` then `[x26, #imm]`.
struct Mode {
  std::optional<unsigned> fixed;
  bool bti = false;
  bool direct() const { return fixed.has_value(); }
};

/// LLVM's opcode and register numbers, looked up by name once at startup from the tables LLVM publishes (public headers only, no generated enums).
struct Names {
  std::map<std::string, unsigned> opcodes;
  std::map<std::string, MCRegister> regs;
  unsigned gpr64sp = ~0u;

  Names(const MCInstrInfo &II, const MCRegisterInfo &RI) {
    for (unsigned k = 0; k < II.getNumOpcodes(); ++k)
      opcodes[II.getName(k).str()] = k;
    for (unsigned k = 1; k < RI.getNumRegs(); ++k)
      regs[RI.getName(k)] = MCRegister(k);
    for (unsigned k = 0; k < RI.getNumRegClasses(); ++k)
      if (StringRef(RI.getRegClassName(&RI.getRegClass(k))) == "GPR64sp")
        gpr64sp = k;
  }
  unsigned op(const std::string &n) const {
    auto f = opcodes.find(n);
    if (f == opcodes.end())
      report_fatal_error(Twine("ferros-as: LLVM has no opcode ") + n);
    return f->second;
  }
  std::optional<unsigned> maybe(const std::string &n) const {
    auto f = opcodes.find(n);
    if (f == opcodes.end())
      return std::nullopt;
    return f->second;
  }
  MCRegister reg(const char *n) const {
    auto f = regs.find(n);
    if (f == regs.end())
      report_fatal_error(Twine("ferros-as: LLVM has no register ") + n);
    return f->second;
  }
};

bool ends(const std::string &s, const char *t) { return StringRef(s).ends_with(t); }
bool starts(const std::string &s, const char *t) { return StringRef(s).starts_with(t); }

/// A pair's immediate is scaled by its register size: the letter after LDP/STP/LDNP/STNP (X and D 8 bytes, W, S and SW 4, Q 16).
int64_t pair_scale(const std::string &name) {
  size_t at = starts(name, "LDNP") || starts(name, "STNP") ? 4 : 3;
  switch (name[at]) {
  case 'X':
  case 'D':
    return 8;
  case 'W':
  case 'S':
    return 4;
  case 'Q':
    return 16;
  }
  return 0;
}

bool is_pair(const std::string &name) { return starts(name, "LDP") || starts(name, "STP") || starts(name, "LDNP") || starts(name, "STNP"); }

/// A register-offset access's shift when it shifts (log2 of the access size), by the stem after LDR/STR (LDRXroX → X, LDRSBWroW → SBW, PRFMroX 8 bytes).
int ro_shift(const std::string &stem) {
  if (stem == "PRFM")
    return 3;
  std::string s = stem.substr(3);
  if (s == "BB" || s == "B" || s == "SBW" || s == "SBX")
    return 0;
  if (s == "HH" || s == "H" || s == "SHW" || s == "SHX")
    return 1;
  if (s == "W" || s == "S" || s == "SW")
    return 2;
  if (s == "X" || s == "D")
    return 3;
  if (s == "Q")
    return 4;
  return -1;
}

/// A structure load or store's post-index immediate when its register is xzr: the bytes it moves (LD1Twov16b_POST 32, LD1Rv16b_POST 1, LD2i32_POST 8), from the opcode's name.
int64_t simd_post_bytes(const std::string &name) {
  int n = name[2] - '0';
  StringRef s(name);
  s = s.drop_back(5);
  // A single lane: LDNi8/16/32/64.
  for (int bits : {8, 16, 32, 64})
    if (s.ends_with(("i" + std::to_string(bits)).c_str()))
      return n * bits / 8;
  auto elem = [](char c) { return c == 'b' ? 1 : c == 'h' ? 2 : c == 's' ? 4 : 8; };
  // Replicate: LDNRv<count><elem>, one element per register.
  size_t v = s.rfind('v');
  if (v == StringRef::npos)
    return 0;
  StringRef arr = s.drop_front(v + 1);
  int count = 0;
  arr.drop_back(1).getAsInteger(10, count);
  int width = count * elem(arr.back());
  if (s.substr(3, 1) == "R")
    return n * elem(arr.back());
  // Multiple structures: LD1 One/Two/Three/Four registers, LD2..4 that many.
  int regs = n;
  if (n == 1)
    regs = s.contains("Fourv") ? 4 : s.contains("Threev") ? 3 : s.contains("Twov") ? 2 : 1;
  return regs * width;
}

class MaskStreamer : public MCELFStreamer {
public:
  MaskStreamer(MCContext &C, std::unique_ptr<MCAsmBackend> B, std::unique_ptr<MCObjectWriter> W, std::unique_ptr<MCCodeEmitter> E, const MCInstrInfo &II, const MCRegisterInfo &RI, MCInstPrinter &P, Mode M, bool dump)
      : MCELFStreamer(C, std::move(B), std::move(W), std::move(E)), II(II), RI(RI), P(P), N(II, RI), M(M), dump(dump) {
    X21 = N.reg("X21");
    X22 = N.reg("X22");
    X23 = N.reg("X23");
    X24 = N.reg("X24");
    X25 = N.reg("X25");
    X26 = N.reg("X26");
    X27 = N.reg("X27");
    X28 = N.reg("X28");
    SP = N.reg("SP");
    XZR = N.reg("XZR");
    LR = N.reg("LR");
  }

  MCAsmParser *parser = nullptr;
  unsigned rewritten = 0;

  // The AArch64 ELF ABI's mapping symbols ($x at the start of each run of instructions, $d of data), as llvm-mc's AArch64ELFStreamer places them (that class lives inside LLVM's .cpp, so its rules are repeated here), so an object matches llvm-mc's byte for byte.
  void changeSection(MCSection *Section, uint32_t Subsection = 0) override {
    last_map[getCurrentSection().first] = map;
    auto it = last_map.find(Section);
    map = it != last_map.end() ? it->second : Map::None;
    MCELFStreamer::changeSection(Section, Subsection);
    if (Section->isText())
      Section->ensureMinAlignment(Align(4));
  }
  void reset() override {
    MCELFStreamer::reset();
    last_map.clear();
    map = Map::None;
  }
  void emitBytes(StringRef Data) override {
    mapping(Map::Data);
    MCELFStreamer::emitBytes(Data);
  }
  void emitValueImpl(const MCExpr *Value, unsigned Size, SMLoc Loc) override {
    mapping(Map::Data);
    MCELFStreamer::emitValueImpl(Value, Size, Loc);
  }
  void emitFill(const MCExpr &NumBytes, uint64_t FillValue, SMLoc Loc) override {
    mapping(Map::Data);
    MCObjectStreamer::emitFill(NumBytes, FillValue, Loc);
  }
  /// `.inst`: a word given as a number, emitted as an instruction and never rewritten (the checker sees it at stow like any other).
  void inst_word(uint32_t w) {
    char b[4];
    for (char &c : b) {
      c = char(uint8_t(w));
      w >>= 8;
    }
    mapping(Map::A64);
    MCELFStreamer::emitBytes(StringRef(b, 4));
  }

  void emitInstruction(const MCInst &I, const MCSubtargetInfo &S) override {
    sti = &S;
    if (dump) {
      show(I);
      return;
    }
    if (rewrite(I))
      ++rewritten;
    else
      out(I);
  }

private:
  const MCInstrInfo &II;
  const MCRegisterInfo &RI;
  MCInstPrinter &P;
  Names N;
  Mode M;
  bool dump;
  const MCSubtargetInfo *sti = nullptr;
  MCRegister X21, X22, X23, X24, X25, X26, X27, X28, SP, XZR, LR;
  enum class Map { None, A64, Data };
  Map map = Map::None;
  std::map<const MCSection *, Map> last_map;

  void mapping(Map m) {
    if (map == m)
      return;
    emitLabel(getContext().createLocalSymbol(m == Map::A64 ? "$x" : "$d"));
    map = m;
  }

  SMLoc here() const { return parser ? parser->getTok().getLoc() : SMLoc(); }

  void refuse(const MCInst &I, const Twine &why) {
    std::string text;
    raw_string_ostream os(text);
    P.printInst(&I, 0, "", *sti, os);
    getContext().reportError(here(), why + ":" + text);
  }

  void out(const MCInst &I) {
    mapping(Map::A64);
    MCELFStreamer::emitInstruction(I, *sti);
  }

  void ins(unsigned opc, std::initializer_list<MCOperand> ops) {
    MCInst I;
    I.setOpcode(opc);
    for (const MCOperand &o : ops)
      I.addOperand(o);
    out(I);
  }
  static MCOperand R(MCRegister r) { return MCOperand::createReg(r); }
  static MCOperand Im(int64_t v) { return MCOperand::createImm(v); }

  /// `add|sub dst, src, #|delta|` (the writeback deltas are at most 1024, inside the 12-bit immediate).
  void add_imm(MCRegister dst, MCRegister src, int64_t delta) {
    if (delta >= 0)
      ins(N.op("ADDXri"), {R(dst), R(src), Im(delta), Im(0)});
    else
      ins(N.op("SUBXri"), {R(dst), R(src), Im(-delta), Im(0)});
  }

  /// `src` made safe: ring mode the masked offset in x26, fixed the address itself in x26.
  void safe(MCRegister src) {
    if (M.fixed)
      ins(N.op("BFMXri"), {R(X26), R(X26), R(src), Im(0), Im(*M.fixed - 1)});
    else
      ins(N.op("ANDXrs"), {R(X26), R(src), R(X27), Im(0)});
  }

  /// sp from x24 through the mode's one sp writer.
  void sp_from_x24() {
    if (M.direct()) {
      safe(X24);
      ins(N.op("ADDXri"), {R(SP), R(X26), Im(0), Im(0)}); // mov sp, x26
    } else {
      ins(N.op("ANDXrs"), {R(X26), R(X24), R(X27), Im(0)});
      ins(N.op("ADDXrx64"), {R(SP), R(X28), R(X26), Im(3 << 3)}); // add sp, x28, x26, uxtx
    }
  }

  void sp_adjust(int64_t delta) {
    add_imm(X24, SP, delta);
    sp_from_x24();
  }

  /// One memory instruction taken apart: the operands before the address, the address's base and what follows it, and the forms to rebuild it in.
  struct Access {
    std::vector<MCOperand> pre; // destination and source registers, without the writeback def
    MCRegister base;
    enum { BaseOnly, Imm, Ro } form;
    std::optional<MCOperand> imm;  // the immediate (or relocation) after the base, if the form has one
    int64_t imm_bytes = 0;         // the immediate in bytes, when it is a number
    bool wb_pre = false, wb_post = false;
    std::optional<int64_t> post_imm; // a writeback's delta in bytes
    MCRegister post_reg;           // a structure post-index by register
    unsigned off_opc = 0;          // the base-plus-immediate (or base only) opcode
    bool off_has_imm = false;
    std::optional<unsigned> ro_opc; // the [x28, x26] opcode, for the forms ferros-mask called ldr/str/ldrb/... (has_reg_offset)
    std::vector<MCOperand> ro_rest; // a register offset's index and extend
    int ro_shift = 0;
  };

  /// `I` as an Access, or a refusal.
  std::optional<Access> take_apart(const MCInst &I, std::string &why) {
    const MCInstrDesc &D = II.get(I.getOpcode());
    std::string name = II.getName(I.getOpcode()).str();
    int b = -1;
    for (unsigned k = D.getNumDefs(); k < I.getNumOperands() && k < D.getNumOperands(); ++k)
      if (D.operands()[k].RegClass == (int16_t)N.gpr64sp && I.getOperand(k).isReg()) {
        b = k;
        break;
      }
    if (b < 0) {
      why = "no base register (a literal load, or a form this assembler does not know)";
      return std::nullopt;
    }
    Access a;
    a.base = I.getOperand(b).getReg();
    bool wb = D.getOperandConstraint(b, MCOI::TIED_TO) == 0;
    for (int k = wb ? 1 : 0; k < b; ++k)
      a.pre.push_back(I.getOperand(k));
    unsigned rest = I.getNumOperands() - b - 1;
    if (wb) {
      if (ends(name, "_POST")) {
        a.wb_post = true;
        a.off_opc = N.op(name.substr(0, name.size() - 5));
        a.form = Access::BaseOnly;
        MCRegister xm = I.getOperand(b + 1).getReg();
        if (xm == XZR)
          a.post_imm = simd_post_bytes(name);
        else
          a.post_reg = xm;
        return a;
      }
      bool pre = ends(name, "pre");
      if (!pre && !ends(name, "post")) {
        why = "writeback in a form this assembler does not know";
        return std::nullopt;
      }
      std::string stem = name.substr(0, name.size() - (pre ? 3 : 4));
      int64_t scale = is_pair(name) ? pair_scale(name) : 1;
      int64_t d = I.getOperand(b + 1).getImm() * scale;
      a.wb_pre = pre;
      a.wb_post = !pre;
      a.post_imm = d;
      a.form = Access::Imm;
      a.off_opc = N.op(stem + (is_pair(name) ? "i" : "ui"));
      a.off_has_imm = true;
      if (!is_pair(name))
        a.ro_opc = N.maybe(stem + "roX");
      return a;
    }
    if (rest == 0) {
      a.form = Access::BaseOnly;
      a.off_opc = I.getOpcode();
      return a;
    }
    if (ends(name, "roX") || ends(name, "roW")) {
      std::string stem = name.substr(0, name.size() - 3);
      a.form = Access::Ro;
      for (unsigned k = b + 1; k < I.getNumOperands(); ++k)
        a.ro_rest.push_back(I.getOperand(k));
      a.ro_shift = ro_shift(stem);
      a.off_opc = N.op(stem + "ui");
      a.off_has_imm = true;
      a.ro_opc = N.maybe(stem + "roX");
      if (a.ro_shift < 0) {
        why = "a register offset this assembler cannot size";
        return std::nullopt;
      }
      return a;
    }
    bool ui = ends(name, "ui");
    bool pair = !ui && ends(name, "i") && is_pair(name);
    bool unscaled = !ui && !pair && ends(name, "i") && StringRef(name).contains("UR") && !starts(name, "LDTR") && !starts(name, "STTR");
    if (rest != 1 || !(ui || pair || unscaled)) {
      why = "an address form this assembler does not know";
      return std::nullopt;
    }
    a.form = Access::Imm;
    a.off_opc = I.getOpcode();
    a.off_has_imm = true;
    const MCOperand &o = I.getOperand(b + 1);
    // An immediate of 0 is no immediate (LLVM prints `[x8]`), as ferros-mask read it; a relocation (:lo12:) always is one.
    if (o.isExpr() || (o.isImm() && o.getImm() != 0)) {
      a.imm = o;
      if (o.isImm())
        a.imm_bytes = o.getImm() * (pair ? pair_scale(name) : 1);
    }
    if (ui)
      a.ro_opc = N.maybe(name.substr(0, name.size() - 2) + "roX");
    return a;
  }

  /// The access itself from `base`, with `imm` (none: 0, or the base-only form).
  void access(const Access &a, MCRegister base, std::optional<MCOperand> imm) {
    MCInst I;
    I.setOpcode(a.off_opc);
    for (const MCOperand &o : a.pre)
      I.addOperand(o);
    I.addOperand(R(base));
    if (a.off_has_imm)
      I.addOperand(imm ? *imm : Im(0));
    out(I);
  }

  /// The access from x26 as the mode wants it: fixed `[x26]`, ring `[x28, x26]` where a register-offset form exists, else through x25.
  void access_safe(const Access &a, std::optional<MCOperand> imm) {
    if (M.direct()) {
      access(a, X26, imm);
    } else if (!imm && a.ro_opc) {
      MCInst I;
      I.setOpcode(*a.ro_opc);
      for (const MCOperand &o : a.pre)
        I.addOperand(o);
      I.addOperand(R(X28));
      I.addOperand(R(X26));
      I.addOperand(Im(0));
      I.addOperand(Im(0));
      out(I);
    } else {
      ins(N.op("ADDXrs"), {R(X25), R(X28), R(X26), Im(0)});
      access(a, X25, imm);
    }
  }

  bool is_reserved(MCRegister r) const { return r == X21 || r == X22 || r == X23 || r == X24 || r == X25 || r == X26 || r == X27 || r == X28; }

  /// Rewrite one instruction; false when it passes through unchanged.
  bool rewrite(const MCInst &I) {
    const MCInstrDesc &D = II.get(I.getOpcode());
    if (M.bti) {
      // Every return becomes `br x30` and every call is followed by `bti j`, so a return lands only right after a call (SANDBOX.md "BTI").
      if (D.isReturn() && I.getNumOperands() == 1 && I.getOperand(0).isReg()) {
        ins(N.op("BR"), {R(I.getOperand(0).getReg())});
        return true;
      }
      if (D.isCall()) {
        out(I);
        ins(N.op("HINT"), {Im(36)}); // bti j
        return true;
      }
    }
    if (D.mayLoad() || D.mayStore()) {
      // Barriers and hints are marked as touching memory but name no address; a literal load names a label instead of a base, and is refused in memory().
      bool addressed = false;
      for (unsigned k = 0; k < I.getNumOperands(); ++k)
        addressed |= (k < D.getNumOperands() && D.operands()[k].RegClass == (int16_t)N.gpr64sp) || I.getOperand(k).isExpr();
      return addressed ? memory(I) : false;
    }
    // A write to sp outside a memory access: computed into x24, then the masked form.
    if (D.getNumDefs() > 0 && I.getNumOperands() > 0 && I.getOperand(0).isReg() && I.getOperand(0).getReg() == SP) {
      unsigned opc = I.getOpcode();
      // The mode's own sp writer, written by hand, passes.
      if (!M.direct() && opc == N.op("ADDXrx64") && I.getOperand(1).getReg() == X28 && I.getOperand(2).getReg() == X26 && I.getOperand(3).getImm() == (3 << 3))
        return false;
      if (M.direct() && opc == N.op("ADDXri") && I.getOperand(1).getReg() == X26 && I.getOperand(2).getImm() == 0 && I.getOperand(3).getImm() == 0)
        return false;
      if (opc == N.op("ADDXri") && I.getOperand(2).getImm() == 0 && I.getOperand(3).getImm() == 0 && I.getOperand(1).getReg() != SP) {
        // `mov sp, xN` as `mov x24, xN`, the alias ferros-mask's text assembled to.
        ins(N.op("ORRXrs"), {R(X24), R(XZR), R(I.getOperand(1).getReg()), Im(0)});
      } else {
        MCInst J = I;
        J.getOperand(0).setReg(X24);
        out(J);
      }
      sp_from_x24();
      return true;
    }
    return false;
  }

  bool memory(const MCInst &I) {
    std::string why;
    std::optional<Access> got = take_apart(I, why);
    if (!got) {
      refuse(I, why);
      return true;
    }
    Access &a = *got;
    std::string name = II.getName(I.getOpcode()).str();
    // The handle forms (SANDBOX.md "Handles"), written by hand in ferros_sys: the pair load from the table, and an access through the pair.
    if (name == "LDPXi" && a.base == X28 && a.pre.size() == 2 && a.pre[0].getReg() == X23 && a.pre[1].getReg() == X22)
      return false;
    if (a.form == Access::Ro && a.base == X23 && a.ro_rest.size() == 3 && a.ro_rest[0].isReg() && a.ro_rest[0].getReg() == X21 && a.ro_rest[1].getImm() == 0 && a.ro_rest[2].getImm() == 0 && ends(name, "roX"))
      return false;
    if (is_reserved(a.base)) {
      refuse(I, "reserved register as base");
      return true;
    }
    if (a.base == SP) {
      if (!a.wb_pre && !a.wb_post) {
        // [sp] and [sp, #imm] stand; a negative offset reaches below the arc base and is folded into the masked address below.
        if (a.form == Access::BaseOnly || (a.form == Access::Imm && (!a.imm || a.imm->isExpr() || a.imm_bytes >= 0)))
          return false;
      } else if (a.post_reg) {
        refuse(I, "sp moved by a register in a structure post-index");
        return true;
      } else if (a.wb_pre) {
        sp_adjust(*a.post_imm);
        access(a, SP, std::nullopt);
        return true;
      } else {
        access(a, SP, std::nullopt);
        sp_adjust(*a.post_imm);
        return true;
      }
    }
    // A general base (or sp with a register offset, or sp below itself).
    if (a.wb_pre)
      add_imm(a.base, a.base, *a.post_imm);
    if (a.form == Access::Imm && a.imm && a.imm->isImm() && a.imm_bytes < 0 && !a.wb_pre) {
      // A negative immediate: the address computed first, masked, then accessed with no offset; the checker admits nothing below the arc base.
      add_imm(X24, a.base, a.imm_bytes);
      safe(X24);
      access_safe(a, std::nullopt);
      return true;
    }
    if (a.form == Access::Ro) {
      // The register offset (with its shift or extend) folded into x24 first, in the form ferros-mask's `add x24, base, index, ext` assembled to.
      MCRegister idx = a.ro_rest[0].getReg();
      bool sign = a.ro_rest[1].getImm() != 0;
      int shift = a.ro_rest[2].getImm() ? a.ro_shift : 0;
      bool w = ends(name, "roW");
      if (w)
        ins(N.op("ADDXrx"), {R(X24), R(a.base), R(idx), Im(((sign ? 6 : 2) << 3) | shift)}); // sxtw / uxtw
      else if (sign || a.base == SP)
        ins(N.op("ADDXrx64"), {R(X24), R(a.base), R(idx), Im(((sign ? 7 : 3) << 3) | shift)}); // sxtx / uxtx (lsl)
      else
        ins(N.op("ADDXrs"), {R(X24), R(a.base), R(idx), Im(shift)}); // lsl
      safe(X24);
    } else {
      safe(a.base);
    }
    access_safe(a, a.wb_pre ? std::nullopt : a.imm);
    if (a.wb_post) {
      if (a.post_reg)
        ins(N.op("ADDXrs"), {R(a.base), R(a.base), R(a.post_reg), Im(0)});
      else
        add_imm(a.base, a.base, *a.post_imm);
    }
    return true;
  }

  void show(const MCInst &I) {
    const MCInstrDesc &D = II.get(I.getOpcode());
    std::string text;
    raw_string_ostream os(text);
    P.printInst(&I, 0, "", *sti, os);
    outs() << II.getName(I.getOpcode()) << "\t" << (D.mayLoad() ? "L" : "-") << (D.mayStore() ? "S" : "-") << (D.isBranch() ? "B" : "-") << (D.isCall() ? "C" : "-") << (D.isReturn() ? "R" : "-") << (D.isIndirectBranch() ? "I" : "-") << "\t";
    for (unsigned k = 0; k < I.getNumOperands(); ++k) {
      const MCOperand &O = I.getOperand(k);
      std::string cls;
      if (k < D.getNumOperands() && D.operands()[k].RegClass >= 0)
        cls = RI.getRegClassName(&RI.getRegClass(D.operands()[k].RegClass));
      int tied = k < D.getNumOperands() ? D.getOperandConstraint(k, MCOI::TIED_TO) : -1;
      if (O.isReg())
        outs() << "r:" << RI.getName(O.getReg());
      else if (O.isImm())
        outs() << "i:" << O.getImm();
      else if (O.isExpr())
        outs() << "e";
      else
        outs() << "?";
      if (!cls.empty())
        outs() << "{" << cls << "}";
      if (tied >= 0)
        outs() << "=" << tied;
      outs() << (k < D.getNumDefs() ? "d" : "") << " ";
    }
    outs() << "\t|" << text << "\n";
  }
};

/// The AArch64 directives' side (`.inst`, `.variant_pcs`, `.aeabi_attribute`), as llvm-mc's AArch64TargetELFStreamer does them; the asm parser reaches it through the streamer and would crash without one.
class MaskTargetStreamer : public AArch64TargetStreamer {
public:
  explicit MaskTargetStreamer(MaskStreamer &S) : AArch64TargetStreamer(S), S(S) {}

  void emitInst(uint32_t Inst) override { S.inst_word(Inst); }
  void emitDirectiveVariantPCS(MCSymbol *Symbol) override {
    S.getAssembler().registerSymbol(*Symbol);
    static_cast<MCSymbolELF *>(Symbol)->setOther(ELF::STO_AARCH64_VARIANT_PCS);
  }
  void emitAttribute(StringRef VendorName, unsigned Tag, unsigned Value, std::string String) override {
    if (unsigned(-1) != Value)
      AArch64TargetStreamer::emitAttribute(VendorName, Tag, Value, "");
    if ("" != String)
      AArch64TargetStreamer::emitAttribute(VendorName, Tag, unsigned(-1), String);
  }
  void finish() override {
    AArch64TargetStreamer::finish();
    S.emitAttributesSection(attributes, ".ARM.attributes", ELF::SHT_AARCH64_ATTRIBUTES, AttributeSubSections);
  }

private:
  MaskStreamer &S;
  MCSection *attributes = nullptr;
};

int fail(const std::string &why) {
  errs() << "ferros-as: " << why << "\n";
  return 2;
}

} // namespace

int main(int argc, char **argv) {
  std::string in, out, mattr;
  bool dump = false;
  Mode M;
  for (int k = 1; k < argc; ++k) {
    std::string a = argv[k];
    if (a == "--dump")
      dump = true;
    else if (a == "--bti")
      M.bti = true;
    else if (a == "--fixed" && k + 1 < argc) {
      unsigned v = std::stoul(argv[++k]);
      if (v < 16 || v > 40)
        return fail("--fixed needs K, 16 to 40");
      M.fixed = v;
    } else if (a == "-o" && k + 1 < argc)
      out = argv[++k];
    else if (a.rfind("-mattr=", 0) == 0)
      mattr = a.substr(7);
    else if (in.empty())
      in = a;
    else
      return fail("unexpected argument " + a);
  }
  if (in.empty() || (!dump && out.empty()))
    return fail("usage: ferros-as <in.s> -o <out.o> [-mattr=...] [--fixed K] [--bti] | ferros-as --dump <in.s>");

  LLVMInitializeAArch64TargetInfo();
  LLVMInitializeAArch64TargetMC();
  LLVMInitializeAArch64AsmParser();
  Triple T("aarch64-unknown-none");
  std::string err;
  const Target *TheTarget = TargetRegistry::lookupTarget(T, err);
  if (!TheTarget)
    return fail(err);

  auto Buf = MemoryBuffer::getFileOrSTDIN(in);
  if (!Buf)
    return fail("cannot read " + in);
  SourceMgr SM;
  SM.AddNewSourceBuffer(std::move(*Buf), SMLoc());

  MCTargetOptions Opts;
  std::unique_ptr<MCRegisterInfo> MRI(TheTarget->createMCRegInfo(T));
  std::unique_ptr<MCAsmInfo> MAI(TheTarget->createMCAsmInfo(*MRI, T, Opts));
  std::unique_ptr<MCSubtargetInfo> STI(TheTarget->createMCSubtargetInfo(T, "", mattr));
  std::unique_ptr<MCInstrInfo> MII(TheTarget->createMCInstrInfo());
  MCContext Ctx(T, MAI.get(), MRI.get(), STI.get(), &SM, &Opts);
  std::unique_ptr<MCObjectFileInfo> MOFI(TheTarget->createMCObjectFileInfo(Ctx, /*PIC=*/false));
  Ctx.setObjectFileInfo(MOFI.get());

  std::error_code EC;
  std::string sink = dump ? "/dev/null" : out;
  auto OS = std::make_unique<raw_fd_ostream>(sink, EC, sys::fs::OF_None);
  if (EC)
    return fail("cannot write " + sink + ": " + EC.message());

  std::unique_ptr<MCInstPrinter> IP(TheTarget->createMCInstPrinter(T, 0, *MAI, *MII, *MRI));
  std::unique_ptr<MCCodeEmitter> CE(TheTarget->createMCCodeEmitter(*MII, Ctx));
  std::unique_ptr<MCAsmBackend> MAB(TheTarget->createMCAsmBackend(*STI, *MRI, Opts));
  auto OW = MAB->createObjectWriter(*OS);
  auto *MS = new MaskStreamer(Ctx, std::move(MAB), std::move(OW), std::move(CE), *MII, *MRI, *IP, M, dump);
  std::unique_ptr<MCStreamer> Str(MS);
  new MaskTargetStreamer(*MS); // owned by the streamer from here
  Str->initSections(false, *STI);

  std::unique_ptr<MCAsmParser> Parser(createMCAsmParser(SM, Ctx, *Str, *MAI));
  std::unique_ptr<MCTargetAsmParser> TAP(TheTarget->createMCAsmParser(*STI, *Parser, *MII, Opts));
  if (!TAP)
    return fail("no AArch64 assembly parser");
  Parser->setTargetParser(*TAP);
  MS->parser = Parser.get();
  bool bad = Parser->Run(false);
  if (bad || Ctx.hadError()) {
    OS->close();
    if (!dump)
      sys::fs::remove(out);
    return 1;
  }
  // Run() finished the object itself (a second finish() wrote every relocation twice).
  if (!dump)
    errs() << in << ": " << MS->rewritten << " instructions rewritten\n";
  return 0;
}
