// TODO(ai-review): review for style and correctness
//
// Shiki grammar for the `text/x-playmaker-fsm` pseudocode the backend's
// PlayMakerFSM dumper emits (`playmakerfsm::pseudo::render` — that
// renderer's output is this grammar's spec):
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
// Scopes picked against github-dark: keywords red, action names and
// property members purple, state names green, events/numbers/constants
// blue, parameter labels orange, strings light-blue, comments and
// `(file)` suffixes gray, undecodable residue pink-italic.

import type { LanguageRegistration } from "shiki/core";

const grammar: LanguageRegistration = {
  // `name` is the id shiki registers and looks the grammar up by; the
  // human-readable label goes in `displayName`.
  name: "playmakerfsm",
  displayName: "PlayMaker FSM",
  scopeName: "source.playmakerfsm",

  patterns: [
    // Structural lines first, anchored to the line start so their
    // keywords only color in place (a param named `start` stays plain).
    { include: "#fsm-header" },
    { include: "#state-header" },
    { include: "#start-line" },
    { include: "#transition-line" },
    { include: "#var-decl" },
    { include: "#action-line" },
    // Value rules ordered so each rule's lookalikes are claimed first:
    // `(unset var)` before the `var` keyword, vectors and raw bytes
    // before numbers, event targets before generic calls.
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
    /// `fsm Bell Shrine {`.
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

    "start-line": {
      match: "^\\s*(start)\\s+(.+?)\\s*$",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "entity.name.tag.state.playmakerfsm" },
      },
    },

    /// `on RESET → Init`; the trailing `// from any state` is left to the
    /// comment rule via the lookahead. Event and state names may contain
    /// spaces, hence the lazy captures.
    "transition-line": {
      match: "^\\s*(on)\\s+(.+?)\\s+(→)\\s+(.+?)\\s*(?=//|$)",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "constant.language.event.playmakerfsm" },
        "3": { name: "keyword.operator.arrow.playmakerfsm" },
        "4": { name: "entity.name.tag.state.playmakerfsm" },
      },
    },

    /// `var On: bool = false` — prefix only; the value is tokenized by
    /// the shared value rules.
    "var-decl": {
      match: "^\\s*(var)\\s+(.+?)\\s*:\\s*([A-Za-z_]\\w*)\\s*(=)",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "variable.parameter.playmakerfsm" },
        "3": { name: "storage.type.playmakerfsm" },
        "4": { name: "keyword.operator.assignment.playmakerfsm" },
      },
    },

    /// An action call at the start of a state body.
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

    /// Placeholder param values; must precede `#var-keyword`, which
    /// would color the `var` in `(unset var)`.
    "missing-value": {
      match: "\\((?:unset var|unset|unused|none)\\)",
      name: "constant.language.missing.playmakerfsm",
    },

    /// Params the decoder couldn't slice, kept as their byte length.
    "raw-bytes": {
      match: "\\(\\d+B\\)",
      name: "invalid.deprecated.playmakerfsm",
    },

    "null-ref": {
      match: "<null>",
      name: "constant.language.null.playmakerfsm",
    },

    /// `Fn(<TypeName>)` — a declared parameter the decoder couldn't read.
    "type-arg": {
      match: "<([A-Za-z_][\\w.]*)>",
      captures: { "1": { name: "support.type.playmakerfsm" } },
    },

    /// An object resolvable to a path id but not a name.
    "loose-ref": {
      match: "\\bloose:\\d+",
      name: "invalid.deprecated.playmakerfsm",
    },

    /// Numeric vectors, `(1, 0.5, 0)`: digits/dots/commas only, so it
    /// never claims the `(file)` suffix of an object reference.
    vector: {
      match: "\\([0-9.,\t -]+\\)",
      name: "constant.numeric.playmakerfsm",
    },

    /// The `(file)` of `Some/Path (sharedassets1.assets)` — matched from
    /// the `(` so the token starts at the paren.
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

    /// `Transform.position on var "Target"` — the member before the `on`.
    "property-member": {
      match: "\\b([A-Za-z_][\\w.]*)\\s+(?=on\\b)",
      captures: { "1": { name: "entity.other.attribute-name.playmakerfsm" } },
    },

    /// Event-target kinds and the owner keyword; a word char, `/` or `:`
    /// before it means it's an object path segment instead.
    "event-target": {
      match:
        "(?<![\\w./:])(?:Self|GameObjectFSM|GameObject|FSMComponent|BroadcastAll|HostFSM|SubFSMs)(?=[\\s(),\\]]|$)",
      name: "support.constant.playmakerfsm",
    },

    /// Enum and layer values, `SomeEnum(2)` / `Default(0)`. A call with a
    /// bare int argument shares this color — the shapes are
    /// indistinguishable in the pseudocode.
    "enum-value": {
      match: "\\b([A-Za-z_]\\w*)\\((\\d+)\\)",
      captures: {
        "1": { name: "support.type.playmakerfsm" },
        "2": { name: "constant.numeric.playmakerfsm" },
      },
    },

    /// `template=bell_shrine`; must precede `#param-label`.
    "template-label": {
      match: "\\b(template)(=)",
      captures: {
        "1": { name: "keyword.other.playmakerfsm" },
        "2": { name: "keyword.operator.assignment.playmakerfsm" },
      },
    },

    /// Template binding-list labels: `in[`, `out[`, `vars[`, `events[`.
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

    /// A template binding's variable, `Bell <- var "On"`; lazy up to
    /// the arrow since names may contain spaces.
    binding: {
      match: "([A-Za-z_][\\w ]*?)(?=\\s*(?:<-|->))",
      captures: { "1": { name: "variable.parameter.playmakerfsm" } },
    },

    /// A named action argument, `boolVariable=`. The name class excludes
    /// punctuation, so a label never spills across a `,` or `(`.
    "param-label": {
      match: "([A-Za-z_][\\w ]*?)(=)",
      captures: {
        "1": { name: "variable.parameter.playmakerfsm" },
        "2": { name: "keyword.operator.assignment.playmakerfsm" },
      },
    },

    /// Any other call, `DoThing(...)` mid-line.
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
