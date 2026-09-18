class Greeter
  attr_reader :name

  def initialize(name)
    @name = name
  end

  def greet(prefix = "Hello")
    "#{prefix}, #{@name}!"
  end
end

people = [Greeter.new("Zcv"), Greeter.new("Ruby")]
people.each { |person| puts person.greet }
