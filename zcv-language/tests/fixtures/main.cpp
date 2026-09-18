#include <string>
#include <vector>

class Greeter {
public:
    explicit Greeter(std::string name) : name_(std::move(name)) {}
    std::string greet() const { return "Hello, " + name_; }
private:
    std::string name_;
};

int main() {
    std::vector<Greeter> people;
    people.emplace_back("Zcv");
    for (const auto &person : people) { return person.greet().size(); }
    return 0;
}
