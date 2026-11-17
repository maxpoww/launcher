# Unicode CLDR's emoji annotations for ONE language -> one table of the daemon's emoji words
# (see gen-emoji-words.sh). Inputs: --slurpfile gem = gemoji's db/emoji.json (which emoji the
# picker has); --slurpfile notes = an array of CLDR annotation files (annotations and
# annotationsDerived, for each locale that makes up the language); --arg name = the table's
# name; --arg doc = its doc line.
def strip: gsub("️"; "");
def notes($a): ($a.annotations.annotations // $a.annotationsDerived.annotations // {});
def once: reduce .[] as $w ([]; if index([$w]) then . else . + [$w] end);
(reduce ($notes[0][] | notes(.) | to_entries[]) as $e ({};
    .[$e.key | strip] += (($e.value.tts // []) + ($e.value.default // []) | map(ascii_downcase)))) as $all
| ($gem[0] | map(.emoji | strip) | unique) as $want
| "",
  "/// " + $doc,
  "pub(crate) static " + $name + ": &[(&str, &str)] = &[",
  ($want[] | select($all[.] != null) | . as $k | "    (" + ($k | @json) + ", " + ($all[$k] | once | join(" ") | @json) + "),"),
  "];"
