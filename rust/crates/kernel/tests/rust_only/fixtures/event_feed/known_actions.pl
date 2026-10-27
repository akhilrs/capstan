undef $/; $t = <STDIN>; $a = q{"([a-z_]+\.[a-z_]+)"}; %s = ();
while ($t =~ /\b(?:mutate|mutate_as_controller)\(\s*[A-Za-z_&.]+,\s*$a/g) { $s{$1} = 1 }
while ($t =~ /\b(?:mutate_as_window_controller|message_mutation|reject)\(\s*[^,]+,\s*[^,]+,\s*$a/g) { $s{$1} = 1 }
while ($t =~ /\bappend_event\(\s*(?:[^,]+,\s*){6}$a/g) { $s{$1} = 1 }
while ($t =~ /\{\s*$a\s*\}\s*else\s*\{\s*$a\s*\}/g) { $s{$1} = 1; $s{$2} = 1 }
print "$_\n" for sort keys %s;
