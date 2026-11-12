// Font-size ratios resolve against each native parent, including live updates.
import { Text, View, signal } from "nsh";

export default function RelativeFontSize() {
  const size = signal(20);
  return (
    <View style={{ fontSize: size(), flexDirection: "column" }}>
      <View style={{ fontSize: "1.5em" }}>
        <Text testID="em-child">Relative child</Text>
        <Text testID="em-grandchild" style={{ fontSize: "0.5em" }}>Nested ratio</Text>
        <Text testID="em-class" class="text-[0.5em]">Class ratio</Text>
      </View>
      <Text onPress={() => size.set(32)}>Resize font parent</Text>
    </View>
  );
}
