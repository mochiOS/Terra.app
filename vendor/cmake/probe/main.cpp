#include <span>
extern "C" int terra_webkit_toolchain_probe();
int main() {
    int values[]{terra_webkit_toolchain_probe()};
    return std::span(values).front() == 42 ? 0 : 1;
}
