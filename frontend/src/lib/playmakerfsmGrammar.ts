// TODO(ai-review): review for style and correctness
//
// Shiki (TextMate) grammar for the `text/x-playmaker-fsm` pseudocode the
// backend's PlayMakerFSM dumper emits (`playmakerfsm::pseudo::render` in the
// playmakerfsm crate — that renderer's output is this grammar's spec):
//
//   fsm Bell Shrine {
//     start Init
//     on RESET → Init  // from any state
//
//     state Init {
//       SetBoolValue(boolVariable=var "Activated", boolValue=true)
//       on FINISHED → Idle
//     }
//
//     var On: bool = false
//   }
//
// Scope choices are made against the `github-dark` theme the viewer ships:
// keywords red (`#f97583`), action names / property members purple
// (`entity.*`), state names green (`entity.name.tag`), events / numbers /
// `Self` blue (`constant.*`, `support.*`), parameter labels orange
// (`variable.parameter`), strings light-blue, comments and the `(file)`
// suffix of object references gray, and undecodable residue (`(3B)` raw
// bytes, `<deleted action>`, `loose:` refs) pink-italic `invalid.deprecated`.
//
// Every rule must translate through shiki's JavaScript regex engine
// (oniguruma-to-es): keep to plain classes, lazy quantifiers and
// lookarounds — no `\G`, backreferences or possessive groups.

import type { LanguageRegistration } from "shiki/core";

const grammar: LanguageRegistration = {
  // `name` is the id shiki registers and looks the grammar up by; the
  // human-readable label goes in `displayName`.
  name: "playmakerfsm",
  displayName: "PlayMaker FSM",
  scopeName: "source.playmakerfsm",

  patterns: [
    // Structural lines first: each is anchored to the line start, so its
    // keywords only color in place (a param named `start` stays plain) and
    // mid-line values can't be mistaken for headers.
    { include: "#fsm-header" },
    { include: "#state-header" },
    { include: "#start-line" },
    { include: "#transition-line" },
    { include: "#var-decl" },
    { include: "#action-line" },
    // Then, in an order where each rule's lookalikes are claimed by an
    // earlier rule: `(unset var)` before the `var` keyword, vectors and
    // raw bytes before numbers, event targets before generic calls.
    { include: "#comment" },
    { include: "#string" },
    { include: "#deleted-action" },
    { include: "#missing-value" },
    { include: "#raw-bytes" },
    { include: "#null-ref" },
    { include: "#type-arg" },
    { include: "#loose-ref" },
    { include: "#vector" },
    { include: "#file-suffix" },
    { include: "#number" },
    { include: "#boolean" },
    { include: "#property-member" },
    { include: "#event-target" },
    { include: "#enum-value" },
    { include: "#template-label" },
    { include: "#list-label" },
    { include: "#var-keyword" },
    { include: "#enum-keyword" },
    { include: "#binding" },
    { include: "#param-label" },
    { include: "#generic-call" },
    { include: "#arrow" },
    { include: "#on-keyword" },
  ],

  repository: {
    /// `fsm Bell Shrine {` — the FSM name is the block's headline.
    "fsm-header": {
      match: "^\\s*(fsm)\\s+(.+?)\\s*(\\{)\\s*$",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "entity.name.type.playmakerfsm" },
        "3": { name: "punctuation.section.block.begin.playmakerfsm" },
      },
    },

    /// `state Init {` and `sequence state Init {`.
    "state-header": {
      match: "^\\s*(sequence\\s+)?(state)\\s+(.+?)\\s*(\\{)\\s*$",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "keyword.other.playmakerfsm" },
        "3": { name: "entity.name.tag.state.playmakerfsm" },
        "4": { name: "punctuation.section.block.begin.playmakerfsm" },
      },
    },

    /// `start Init`.
    "start-line": {
      match: "^\\s*(start)\\s+(.+?)\\s*$",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "entity.name.tag.state.playmakerfsm" },
      },
    },

    /// `on RESET → Init`, with the trailing `// from any state` of global
    /// transitions left to the comment rule via the lookahead. Event and
    /// state names can contain spaces, so both are lazy up to their
    /// separators.
    "transition-line": {
      match: "^\\s*(on)\\s+(.+?)\\s+(→)\\s+(.+?)\\s*(?=//|$)",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "constant.language.event.playmakerfsm" },
        "3": { name: "keyword.operator.arrow.playmakerfsm" },
        "4": { name: "entity.name.tag.state.playmakerfsm" },
      },
    },

    /// `var On: bool = false` — only the prefix; the value is tokenized
    /// by the shared value rules below the anchored ones.
    "var-decl": {
      match: "^\\s*(var)\\s+(.+?)\\s*:\\s*([A-Za-z_]\\w*)\\s*(=)",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "variable.parameter.playmakerfsm" },
        "3": { name: "storage.type.playmakerfsm" },
        "4": { name: "keyword.operator.assignment.playmakerfsm" },
      },
    },

    /// An action call at the start of a state body: `SetBoolValue(`.
    "action-line": {
      match: "^\\s*([A-Za-z_]\\w*)\\s*\\(",
      captures: { "1": { name: "entity.name.function.playmakerfsm" } },
    },

    comment: {
      match: "//.*",
      name: "comment.line.double-slash.playmakerfsm",
    },

    /// JSON-quoted strings (variable/event references, string values),
    /// with serde_json's escape forms.
    string: {
      begin: '"',
      beginCaptures: { "0": { name: "punctuation.definition.string.begin.playmakerfsm" } },
      end: '"',
      endCaptures: { "0": { name: "punctuation.definition.string.end.playmakerfsm" } },
      name: "string.quoted.double.playmakerfsm",
      patterns: [
        {
          match: '\\\\(?:["\\\\/bfnrt]|u[0-9a-fA-F]{4})',
          name: "constant.character.escape.playmakerfsm",
        },
      ],
    },

    /// An action whose script class is gone from the build.
    "deleted-action": {
      match: "<deleted action>",
      name: "invalid.deprecated.playmakerfsm",
    },

    /// Placeholder param values. Before `#var-keyword`, which would
    /// otherwise color the `var` in `(unset var)`.
    "missing-value": {
      match: "\\((?:unset var|unset|unused|none)\\)",
      name: "constant.language.missing.playmakerfsm",
    },

    /// Params the decoder couldn't slice — kept as their byte length.
    "raw-bytes": {
      match: "\\(\\d+B\\)",
      name: "invalid.deprecated.playmakerfsm",
    },

    "null-ref": {
      match: "<null>",
      name: "constant.language.null.playmakerfsm",
    },

    /// A `Fn(<TypeName>)` call whose declared parameter the decoder
    /// couldn't read.
    "type-arg": {
      match: "<([A-Za-z_][\\w.]*)>",
      captures: { "1": { name: "support.type.playmakerfsm" } },
    },

    /// An object the dump can resolve to a path id but not a name.
    "loose-ref": {
      match: "\\bloose:\\d+",
      name: "invalid.deprecated.playmakerfsm",
    },

    /// Numeric vectors: `(1, 0.5, 0)`. Digits/dots/commas only, so this
    /// never claims the `(file)` suffix of an object reference.
    vector: {
      match: "\\([0-9.,\t -]+\\)",
      name: "constant.numeric.playmakerfsm",
    },

    /// The `(file)` of `Some/Path (sharedassets1.assets)` — always a
    /// space-separated paren group with letters inside, after vectors and
    /// raw bytes have claimed their own paren groups. Matched from the
    /// `(` (lookbehind for the space) so the token starts at the paren.
    "file-suffix": {
      match: "(?<= )\\([^()\\n]*[A-Za-z][^()\\n]*\\)",
      name: "comment.playmakerfsm",
    },

    number: {
      match: "(?<![\\w.])-?(?:\\d+(?:\\.\\d+)?|Infinity|NaN)(?![\\w.])",
      name: "constant.numeric.playmakerfsm",
    },

    boolean: {
      match: "\\b(?:true|false)\\b",
      name: "constant.language.boolean.playmakerfsm",
    },

    /// `Transform.position on var "Target"` — the property before the
    /// `on` (which `#on-keyword` colors further down).
    "property-member": {
      match: "\\b([A-Za-z_][\\w.]*)\\s+(?=on\\b)",
      captures: { "1": { name: "entity.other.attribute-name.playmakerfsm" } },
    },

    /// Event-target kinds and the owner keyword. Word chars, `/` or `:`
    /// before the name would mean it's an object path segment instead.
    "event-target": {
      match:
        "(?<![\\w./:])(?:Self|GameObjectFSM|GameObject|FSMComponent|BroadcastAll|HostFSM|SubFSMs)(?=[\\s(),\\]]|$)",
      name: "support.constant.playmakerfsm",
    },

    /// Enum and layer values: `SomeEnum(2)`, `Default(0)`. Runs before
    /// `#generic-call`, so a call with a bare int argument shares this
    /// color — the two shapes are indistinguishable in the pseudocode.
    "enum-value": {
      match: "\\b([A-Za-z_]\\w*)\\((\\d+)\\)",
      captures: {
        "1": { name: "support.type.playmakerfsm" },
        "2": { name: "constant.numeric.playmakerfsm" },
      },
    },

    /// `template=bell_shrine` — before `#param-label`, which would color
    /// the word like any other `name=` argument.
    "template-label": {
      match: "\\b(template)(=)",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "keyword.operator.assignment.playmakerfsm" },
      },
    },

    /// The binding lists of a template control: `in[`, `out[`,
    /// `vars[`, `events[`.
    "list-label": {
      match: "\\b(?:in|out|vars|events)(?=\\[)",
      name: "keyword.other.playmakerfsm",
    },

    /// Variable references: `var "Activated"`.
    "var-keyword": {
      match: "\\bvar\\b",
      name: "keyword.other.playmakerfsm",
    },

    /// `enum(3)` — an enum-valued variable default.
    "enum-keyword": {
      match: "\\benum(?=\\()",
      name: "keyword.other.playmakerfsm",
    },

    /// A template binding's variable: `Bell <- var "On"`, `Done ->
    /// FINISHED`, `Ticks = 3`. Names may contain spaces, so the capture
    /// is lazy up to the arrow or `=` (which `#param-label` claims).
    binding: {
      match: "([A-Za-z_][\\w ]*?)(?=\\s*(?:<-|->))",
      captures: { "1": { name: "variable.parameter.playmakerfsm" } },
    },

    /// A named action argument: `boolVariable=`. The name class excludes
    /// punctuation, so a label never spills across a `,` or `(`.
    "param-label": {
      match: "([A-Za-z_][\\w ]*?)(=)",
      captures: {
        "1": { name: "variable.parameter.playmakerfsm" },
        "2": { name: "keyword.operator.assignment.playmakerfsm" },
      },
    },

    /// Any other call: `DoThing(...)` mid-line, after enum values and
    /// event targets have claimed their lookalikes.
    "generic-call": {
      match: "\\b([A-Za-z_]\\w*)(?=\\()",
      captures: { "1": { name: "entity.name.function.playmakerfsm" } },
    },

    /// Event params (`→"FINISHED"`) and template binding arrows.
    arrow: {
      match: "→|<-|->",
      name: "keyword.operator.arrow.playmakerfsm",
    },

    /// The `on` of a property reference, mid-line.
    "on-keyword": {
      match: "\\bon\\b",
      name: "keyword.other.playmakerfsm",
    },
  },
};

export default grammar;
