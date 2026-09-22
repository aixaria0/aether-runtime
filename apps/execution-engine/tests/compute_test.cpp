#include "../src/compute.hpp"
#include <cassert>
int main() {
 assert(compute("echo","abc").output == "abc");
 assert(compute("uppercase","abc").output == "ABC");
 assert(compute("sha256","abc").output == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
 assert(!compute("shell","abc").success);
 assert(!compute("echo",std::string(65537,'x')).success);
}
