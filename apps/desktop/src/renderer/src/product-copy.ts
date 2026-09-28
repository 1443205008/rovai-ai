export function localizeExecutionEngineTerms(value: string): string {
  return value
    .replaceAll('Adapter Installation', '智能体')
    .replaceAll('Agent Runtime', '智能体')
    .replaceAll('Runtime Adapter', '智能体适配器')
    .replaceAll('Runtime', '智能体')
    .replaceAll('Adapter', '适配器')
}
