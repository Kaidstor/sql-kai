import { CompletionContext, type CompletionSource } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import { PostgreSQL, schemaCompletionSource, sql, type SQLConfig } from "@codemirror/lang-sql";
import { EditorState } from "@codemirror/state";

export function nestedSchemaCompletionSource(config: SQLConfig): CompletionSource {
  const source = schemaCompletionSource(config);
  const language = sql({ dialect: config.dialect ?? PostgreSQL });
  return async (context) => {
    const node = syntaxTree(context.state).resolveInner(context.pos, -1);
    for (let scope: typeof node | null = node; scope; scope = scope.parent) {
      if (scope.name !== "Parens") continue;
      const first = scope.firstChild?.nextSibling;
      if (first?.name !== "Keyword" || !/^(select|with)$/i.test(context.state.sliceDoc(first.from, first.to))) continue;
      // CodeMirror only collects aliases from direct children of a Statement.
      const from = scope.from + 1;
      const to = scope.lastChild?.name === ")" ? scope.lastChild.from : scope.to;
      const state = EditorState.create({ doc: context.state.sliceDoc(from, to), extensions: [language] });
      const result = await source(new CompletionContext(state, context.pos - from, context.explicit));
      if (result) return { ...result, from: result.from + from, ...(result.to === undefined ? {} : { to: result.to + from }) };
    }
    return source(context);
  };
}
