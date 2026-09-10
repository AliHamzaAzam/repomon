import Select from "./Select";
export type TranscriptDetail = "summary" | "normal" | "verbose";
export default function TranscriptDetailToggle(props: { value: TranscriptDetail; onChange: (value: TranscriptDetail) => void }) {
  return <div class="pointer-events-auto shrink-0 font-sans normal-case tracking-normal" title="Summary: collapsed work, no notices. Normal: collapsed work and limits. Verbose: expanded work and all notices. Settings can override notices per agent.">
    <Select ariaLabel="Transcript detail" value={props.value} align="right" variant="frameless" options={[
      {value:"summary", label:"Summary detail"}, {value:"normal", label:"Normal detail"}, {value:"verbose", label:"Verbose detail"},
    ]} onChange={(value) => props.onChange(value as TranscriptDetail)} />
  </div>;
}
