// STEM. A formula arrives as Typst maths that `adoc-math` converted from AsciiMath or LaTeX and
// checked to hold nothing but maths: Typst's maths functions and symbols, and the helpers below,
// which its LaTeX conversion calls and Typst's maths does not have. Their names are
// `adoc_math::HELPERS`.

#let adoc-math-scope = {
  let delimited(delimiter) = (..rows) => math.mat(delim: delimiter, ..rows.pos())
  let named(name, limits: false) = math.op(name, limits: limits)
  let over(symbol) = body => math.attach(math.limits(symbol), t: body)
  let classed(class) = body => math.class(class, body)
  let same = body => body
  (
    "Bmatrix": delimited("{"),
    "Set": body => math.lr($\{ #body \}$),
    "Vmatrix": delimited("||"),
    "aligned": same,
    "arcctg": named("arcctg"),
    "arctg": named("arctg"),
    "argmax": named("arg max", limits: true),
    "argmin": named("arg min", limits: true),
    "bcancel": body => math.cancel(body, inverted: true),
    "big": same,
    "Big": same,
    "bigg": same,
    "Bigg": same,
    "bmatrix": delimited("["),
    "boxed": body => box(stroke: 0.5pt, inset: (x: 2pt, y: 2pt), $#body$),
    "bra": body => math.lr($angle.l #body |$),
    "braket": body => math.lr($angle.l #body angle.r$),
    "cfrac": (numerator, denominator) => math.display(math.frac(numerator, denominator)),
    "ch": named("ch"),
    "cosec": named("cosec"),
    "cotg": named("cotg"),
    "cth": named("cth"),
    "dbinom": (n, k) => math.display(math.binom(n, k)),
    "dfrac": (numerator, denominator) => math.display(math.frac(numerator, denominator)),
    "injlim": named("inj lim", limits: true),
    "ket": body => math.lr($| #body angle.r$),
    "mathbin": classed("binary"),
    "mathclap": same,
    "mathclose": classed("closing"),
    "mathinner": same,
    "mathop": body => math.op(body),
    "mathopen": classed("opening"),
    "mathord": classed("normal"),
    "mathpunct": classed("punctuation"),
    "mathrel": classed("relation"),
    "matrix": delimited(none),
    "middle": same,
    "mitexbold": math.bold,
    "mitexcal": math.cal,
    "mitexcolor": (colour, body) => body,
    "mitexdisplay": math.display,
    "mitexfrak": math.frak,
    "mitexinline": math.inline,
    "mitexitalic": math.italic,
    "mitexmathbf": body => math.upright(math.bold(body)),
    "mitexmono": math.mono,
    "mitexoverbrace": body => math.limits(math.overbrace(body)),
    "mitexoverbracket": body => math.limits(math.overbracket(body)),
    "mitexsans": math.sans,
    "mitexscript": math.script,
    "mitexsscript": math.sscript,
    "mitexunderbrace": body => math.limits(math.underbrace(body)),
    "mitexunderbracket": body => math.limits(math.underbracket(body)),
    "mitexupright": math.upright,
    "negmedspace": h(-2em / 9),
    "negthickspace": h(-5em / 18),
    "negthinspace": h(-1em / 6),
    "overset": (top, body) => math.attach(math.limits(body), t: top),
    "phantom": body => hide(body),
    "plim": named("plim", limits: true),
    "pmatrix": delimited("("),
    "pmod": n => $quad (mod #n)$,
    "projlim": named("proj lim", limits: true),
    "rcases": (..rows) => math.cases(reverse: true, ..rows.pos()),
    "sect": named("sect"),
    "set": body => math.lr($\{ #body \}$),
    "sh": named("sh"),
    "smallint": sym.integral,
    "stackrel": (top, body) => math.attach(math.limits(body), t: top),
    "tbinom": (n, k) => math.inline(math.binom(n, k)),
    "tfrac": (numerator, denominator) => math.inline(math.frac(numerator, denominator)),
    "th": named("th"),
    "thickapprox": sym.approx,
    "thicksim": sym.tilde.op,
    "underbar": math.underline,
    "underset": (bottom, body) => math.attach(math.limits(body), b: bottom),
    "varDelta": math.italic(sym.Delta),
    "varGamma": math.italic(sym.Gamma),
    "varLambda": math.italic(sym.Lambda),
    "varOmega": math.italic(sym.Omega),
    "varPhi": math.italic(sym.Phi),
    "varPi": math.italic(sym.Pi),
    "varPsi": math.italic(sym.Psi),
    "varSigma": math.italic(sym.Sigma),
    "varTheta": math.italic(sym.Theta),
    "varUpsilon": math.italic(sym.Upsilon),
    "varXi": math.italic(sym.Xi),
    "vmatrix": delimited("|"),
    "xLeftarrow": over(sym.arrow.l.double.long),
    "xLeftrightarrow": over(sym.arrow.l.r.double.long),
    "xRightarrow": over(sym.arrow.r.double.long),
    "xleftarrow": over(sym.arrow.l.long),
    "xleftrightarrow": over(sym.arrow.l.r.long),
    "xlongequal": over(sym.eq),
    "xmapsto": over(sym.arrow.r.long.bar),
    "xrightarrow": over(sym.arrow.r.long),
  )
}

// A formula: `source` is checked Typst maths, `alt` the formula as written, its text
// alternative in a tagged PDF. Equations are set in STIX Two Math, which has the MATH table
// Typst needs.
//
// `eval` is safe here: the emitter passes only `adoc_math::TypstMath`, markup `is_safe` checked
// to hold no code and no name outside Typst's maths and this scope, as a string literal.
#let adoc-stem(source, alt, block: false) = {
  show math.equation: set text(font: "STIX Two Math")
  math.equation(block: block, alt: alt, eval(source, mode: "math", scope: adoc-math-scope))
}
