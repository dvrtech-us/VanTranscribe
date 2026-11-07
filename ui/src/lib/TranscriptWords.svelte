<script lang="ts">
  import type { Segment, SpeakerAssignment } from './api';
  let { segment, assignment }: { segment: Segment; assignment?: SpeakerAssignment } = $props();
  const parts = $derived.by(() => {
    if (!assignment?.words.length || !segment.words.length) return null;
    const result: { text: string; speaker?: string; uncertain?: boolean }[] = [];
    const speakers = new Map(assignment.words.map((word) => [word.index, word]));
    let cursor = 0;
    for (let i = 0; i < segment.words.length; i++) {
      const word = segment.words[i].text.trim();
      if (!word) return null;
      const found = segment.text.indexOf(word, cursor);
      if (found < 0) return null;
      if (found > cursor) result.push({ text: segment.text.slice(cursor, found) });
      const speaker = speakers.get(i);
      result.push({ text: segment.text.slice(found, found + word.length), speaker: speaker?.label, uncertain: speaker?.uncertain || speaker?.overlap });
      cursor = found + word.length;
    }
    if (cursor < segment.text.length) result.push({ text: segment.text.slice(cursor) });
    return result;
  });
</script>
{#if parts}{#each parts as part}{#if part.speaker}<span class:uncertain-word={part.uncertain} title={`Speaker: ${part.speaker}${part.uncertain ? ' · unconfirmed' : ''}`}>{part.text}</span>{:else}{part.text}{/if}{/each}{:else}{segment.text}{/if}
<style>.uncertain-word{text-decoration:underline dotted;text-underline-offset:3px;text-decoration-color:var(--muted)}</style>
