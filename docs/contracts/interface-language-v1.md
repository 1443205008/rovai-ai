---
document_type: interface-contract
contract: interface-language
version: 1
status: accepted
authority: interface-language-preference-and-presentation
source_version: v1.71
last_updated: 2026-09-27
---

# Interface Language v1

`GeneralPreferencesSnapshot` schema 5 adds `interfaceLanguage: 'zh-CN' | 'en'` to the exact schema 4 field set. A new profile and valid schema 1–4 preferences resolve to `zh-CN`; migration retains every other recognized preference. An invalid language or unexpected schema 5 field follows the existing invalid-preference degradation path and preserves the original file. Electron Main owns the serialized private write; Desktop Preload and the Web presentation adapter expose the same typed `get` and `setInterfaceLanguage` API. Web stores only its local presentation preference.

Selecting a language updates App-owned copy immediately and persists the choice through that API. Earlier save responses cannot replace a newer selection. If the latest save fails, the display returns to the most recently saved language and shows a local error. The language change does not reload or remount the App, change navigation, save a draft, or cancel a task or Run. The document language follows the displayed choice.

The catalog contains only App-owned navigation, controls, explanatory copy, status and error shells, and accessible names. User-written text, saved member profiles, draft content, messages, memories, Runtime output, paths, commands, model names and IDs retain their original bytes. The four built-in member candidates use language-specific initial text before creation; only the selected candidate is saved through the existing onboarding command. Once saved, its identity fields are ordinary member data and are never rewritten by a language change. The durable first-run Camp title follows the existing [First-run Onboarding v5](first-run-onboarding-v5.md) contract.

## References

- [First-run presentation](../ui/components/first-run-onboarding.md)
- [App Shell and General Settings](../ui/components/app-shell-navigation.md)
- [First-run Onboarding v5](first-run-onboarding-v5.md)
