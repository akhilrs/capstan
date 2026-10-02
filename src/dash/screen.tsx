import { Box, Text } from "ink";
import type { Line, Span } from "./lines.js";
import type { Overlay } from "./overlays.js";

function styleProps(s: Span) {
  return {
    ...(s.color === undefined ? {} : { color: s.color }),
    ...(s.bg === undefined ? {} : { backgroundColor: s.bg }),
    ...(s.bold === true ? { bold: true } : {}),
    ...(s.dim === true ? { dimColor: true } : {}),
  };
}

/** Paints lines of spans; the lines already have their final width, so nothing here wraps or measures. */
export function Lines({ lines }: { lines: readonly Line[] }) {
  return (
    <Box flexDirection="column">
      {lines.map((line, row) => (
        <Text key={row} wrap="truncate">
          {line.map((s, i) => (
            <Text key={i} {...styleProps(s)}>
              {s.text}
            </Text>
          ))}
        </Text>
      ))}
    </Box>
  );
}

/** A box floated over the dashboard. Every overlay line is padded to the box width, so it overwrites the cells beneath it. */
export function FloatingBox({ overlay }: { overlay: Overlay }) {
  return (
    <Box
      position="absolute"
      marginTop={overlay.top}
      marginLeft={overlay.left}
      flexDirection="column"
    >
      <Lines lines={overlay.lines} />
    </Box>
  );
}
